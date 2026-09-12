# CLAUDE.md — Sequin

Guidance for Claude Code working in this repo. Read PLAN.md for the full
roadmap; this file is the operating manual.

## What Sequin is

A macOS desktop app (Dioxus) that fixes studio photo deliveries before import
into Apple Photos. Deliveries arrive with random UUID filenames and identical
EXIF capture times, so the Photos timeline shows them in arbitrary order.
Sequin: (1) groups the styled variants of each shot via perceptual hashing,
(2) lets the user drag groups/photos into shoot order, (3) writes sequential
EXIF capture times so the import sorts correctly. Apple Photos cannot fix
this after import (its batch date adjust only applies a uniform shift) — that
is the entire reason this app exists.

Owner: Rex (rexlorenzo). Personal tool used ~twice a year, published as MIT
open source; possibly a paid Mac App Store convenience build later (Maccy
model). Keep the codebase clean enough for strangers to read.

## Workspace layout

- `crates/sequin-core` — headless engine: hashing, clustering, timeline,
  EXIF. **Never add GUI or Dioxus deps here.** Everything testable without a
  display.
- `crates/sequin-cli` — thin CLI (`sequin group <dir>`, `sequin apply
  <arrangement.json> <start> [--dry-run]`). Reference for how the GUI calls
  the core; also the golden-test harness.
- `crates/sequin-app` — Dioxus 0.7 desktop app. Excluded from
  `default-members`, so plain `cargo build` / `cargo test` never needs
  webview deps. Run it with `cargo run -p sequin-app` (or `dx serve -p
  sequin-app`) on macOS only.

## Commands

```sh
cargo test                 # core unit tests (fast, no GUI deps)
cargo build --release      # core + cli
./target/release/sequin group <dir> > arrangement.json
./target/release/sequin apply arrangement.json "2026-07-18 10:00" --dry-run
```

Quality gates (all must pass before pushing; CI enforces them on macOS +
Linux runners, and `.githooks/pre-commit` runs the fast subset — fresh
clones must run `git config core.hooksPath .githooks` once):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run          # or cargo test
typos                      # brew install typos-cli
shellcheck packaging/macos/*.sh scripts/*.sh .githooks/pre-commit
actionlint                 # workflow YAML + shellcheck over `run:` blocks
python3 -m doctest scripts/golden_check.py   # golden-check self-test
cargo deny check           # advisories/licenses/bans; config in deny.toml
```

Golden test (run whenever touching hashing/grouping code): group the local
test delivery and compare against `fixtures/expected_groups_archive1-2.json`
(sorted filename sets must match EXACTLY — 34 groups from 62 photos). The
photos live only on Rex's Mac; the folder is currently `~/Downloads/Archive1`
(the fixture name still carries the older `Archive1-2` spelling):

```sh
./target/release/sequin group ~/Downloads/Archive1 > /tmp/actual.json
python3 scripts/golden_check.py /tmp/actual.json      # pass/fail + group-set diff

# Anything touching the decode path must pass BOTH ways:
SEQUIN_FULL_DECODE=1 ./target/release/sequin group ~/Downloads/Archive1 > /tmp/full.json
python3 scripts/golden_check.py /tmp/full.json
```

## Validated invariants — do NOT change without re-running the golden test

These were derived and visually verified on a real 62-photo delivery
(2026-07-19). They are settled; don't re-derive or "improve" them blind:

1. **Hash**: custom pHash in `sequin-core/src/hashing.rs` — grayscale →
   64×64 (Lanczos3) → 2D DCT-II → top-left 16×16 coefficients → bit =
   coeff > median. 256 bits. Same construction as Python
   `imagehash.phash(img, hash_size=16)`.
2. **`image_hasher` crate is rejected** — its `preproc_dct()` (32×32 DCT +
   mean threshold) merged 51/62 real photos into one cluster. Do not swap it
   back in.
3. **Two hashes per photo** (full frame + uniform-border-trimmed copy);
   pairwise distance = min of the two.
4. **Cluster threshold 60/256** (union-find over pairs). Real-photo band,
   re-measured on the 62-photo delivery 2026-09-08 with the current Rust
   code: **worst true-variant pair 34, nearest false pair 82** — 26 bits of
   headroom below the threshold, 22 above. Identical on both decode paths.
   (This file previously recorded "≥ 102" for the false pair; that figure
   came from the Python prototype and does not match what the Rust
   implementation measures. 82 is still a wide separation — the point of the
   invariant — but use the measured number.)
5. **Known limitation**: pHash does not match alternate crops or collage
   composites — the GUI must let users drag strays into groups manually.
   This is by design, not a bug to fix in the hasher.
6. **Timestamps**: `DateTimeOriginal` + `CreateDate` + `ModifyDate` written
   together (Photos sorts by DateTimeOriginal), EXIF string format
   `YYYY:MM:DD HH:MM:SS`, naive local time (no timezone handling — that's
   what Photos uses). Defaults: 60s between groups, 10s within a group.

## Hard rules

- **No photos in git, ever.** `.gitignore` blocks `*.jpg/jpeg/png/heic`;
  keep it that way. The fixture contains filenames only. This is a privacy
  rule, not a housekeeping preference.
- **Never write EXIF to originals in tests.** Tests use synthetic images or
  temp copies. The `apply` command's default UX should evolve toward
  copy-to-output-folder (PLAN M4) rather than in-place writes.
- Keep `sequin-core` free of `unwrap()` on user data paths — errors surface
  in the GUI; use `anyhow::Context`.
- Commit `Cargo.lock` (binary app convention).

## Crate/API notes (learned the hard way)

- `little_exif` 0.6: `Metadata::new_from_path` ERRORS on files with no EXIF
  segment — fall back to `Metadata::new()` (already done in `exif.rs`).
  Iterate tags with `for tag in &metadata` (IntoIterator; there is no
  `.data()` method). Read-back values may be NUL-padded — trim `\0`.
- Synthetic test images must differ in LOW-frequency structure to be
  distinguishable by pHash — high-frequency noise/texture differences hash
  identically. See the scene generators in `grouping.rs` tests; measured
  synthetic band is variant≈96 / negative≈124 (worse than real photos).
- Dioxus 0.7: `dioxus::launch(app)`, `use_signal`, `spawn` +
  `tokio::task::spawn_blocking` for heavy work off the UI thread. Pin the
  minor version; 0.x API churn is real.
- `rfd` for native folder pickers (async).
- Icon Composer's renderer (CoreSVG) **ignores `fill-rule`** and fills each
  subpath independently, so an SVG layer cannot express subtraction. Use an
  SVG `<mask>`; even-odd is wrong anyway when the knockout extends past the
  shape it cuts. The layered icon's PNGs are rasterised from masked SVGs in
  `assets/icon-src/layers/`.
- `Assets.car` (the macOS 26 layered icon) is **committed**, not built in CI:
  `actool` output is not reproducible (build timestamp + per-rendition UUIDs),
  and the release runner is `macos-14`, which has no Xcode 26 anyway. Because
  the catalogue cannot be diffed, `make_icon.sh --check` compares a digest of
  its inputs kept in `Assets.car.inputs`. `sign_notarize.sh` only copies it in
  and sets `CFBundleIconName`, before signing — a resource added after
  `codesign` breaks the seal.
- `librsvg` (`rsvg-convert`, used by `scripts/make_icon.sh`) does **not** parse
  CSS `oklch()` — it drops the fill silently, so an oklch-coloured SVG
  rasterises to an empty shape. The icon masters in
  `crates/sequin-app/assets/icon-src/` therefore carry sRGB hex with the
  DESIGN.md token in a comment above each fill. Touching a master means
  re-running `./scripts/make_icon.sh`; `--check` verifies the committed
  `icon.icns`/`icon.png` still match (local/release-time only, not CI —
  antialiasing varies with the cairo version).

## Design context

Before any UI work in `sequin-app`, read `PRODUCT.md` (register: product;
users, personality, anti-references, WCAG AA target) and `DESIGN.md` (seed
visual system: "The Light Table" — photos-first, pure neutral surfaces per
macOS appearance, honey-gold accent ≤10%, sans + mono-for-data type).
Both were created with `/impeccable init`; re-run `/impeccable document`
after M2 to capture real tokens.

## Current state / next work

M1–M5 are DONE. The core flow (group → arrange → write) is complete and
validated, and **v0.1.0 is tagged** at `e543bdd` (2026-09-12). The tag run
built universal (arm64 + x86_64), signed, notarized (Apple `Accepted`),
stapled and verified, and attached `Sequin_0.1.0_universal.dmg` to a **draft**
GitHub release.

**The only thing left is publishing that draft**, which is the maintainer's
call and needs two checks first: a smoke test of *this* build (the
2026-09-11 one passed but ran against the arm64 build, before the new icon),
and a look at the icon in Finder and the Dock — this is the first bundle to
actually carry `Assets.car`, so the first chance to confirm macOS 26 uses the
layered icon instead of falling back to the `.icns`. README screenshots are
still outstanding too.

Do not re-tag or publish without being asked. Note the tag was moved once
already: it sat at `ab85bf1` (pre-icon) and was re-pointed after PR #19, with
the older draft deleted — neither had ever been published.

The scaled-decode golden test that gated all this is **done** (2026-09-08,
both paths pass). See PLAN.md M5.

M3/M4 notes: `sequin-core/src/arrange.rs` is the arrangement model
(reorder, merge, split; serializes to the `arrangement.json` sidecar shared
with the CLI) and `sequin-core/src/apply.rs` is the copy-and-stamp engine
(copy to `sequin-output/` by default, EXIF written to the copies, per-file
failures reported; `--in-place` is an explicit opt-in). The app hosts the
drag/keyboard editor with 100-deep undo and the confirm → progress →
failure-report write dialog.

M2 notes: `sequin-core/src/thumbs.rs` fuses hashing + thumbnailing (one
decode per photo via `hashing::hash_photo_with_work`; ~512px JPEGs cached in
`dirs::cache_dir()/sequin/thumbs/<dir-hash>/`; per-file failures reported,
not fatal). The app serves thumbs via `use_asset_handler("thumbs", …)` and
streams scan progress over a tokio channel into a `Phase` signal. Dev hook:
`SEQUIN_OPEN=<folder> cargo run -p sequin-app` auto-opens a folder on launch
(used for screenshot iteration).

Scan cost (both knobs measured on 62 synthetic 24MP JPEGs):
- **Bounded pool.** Scans run on a rayon pool capped at 4 workers
  (`hashing::scan_threads`); peak RSS scales linearly with concurrent decodes,
  so 10 threads cost 1209 MB / 3.00 s vs 498 MB / 3.25 s at 4. Override with
  `SEQUIN_SCAN_THREADS=<n>`. Scheduling only — never changes hash values.
- **Scaled JPEG decode — ON by default; `SEQUIN_FULL_DECODE=1` restores the
  original path.** `hashing::decode_jpeg_scaled` uses
  `jpeg-decoder`'s reduced IDCT to decode straight to ~`WORK_SIZE` (a 24MP
  frame at 1/4 = ~4.5 MB, not 72 MB); `image`'s zune-jpeg backend has no
  scaling API. PNG, CMYK and 16-bit greyscale fall back to the full decode.
  Net at 4 threads on 62×24MP: baseline JPEG **499 MB / 3.75 s →
  111 MB / 1.26 s**, progressive JPEG **770 MB / 5.21 s → 373 MB / 3.72 s**.
  Progressive must buffer every coefficient before the IDCT, so it saves
  less — size the thread cap against that number, not the baseline one.
  ✅ **Validated on real photos 2026-09-08** — the golden test passes on
  BOTH paths (34 groups / 62 photos each), and the distance bands come out
  byte-identical: worst true-variant pair 34, nearest false pair 82. Real
  drift between the paths is max 4 bits of 256 (mean 0.4) on `hash_full`,
  an order of magnitude under the 12 bits measured synthetically —
  synthetic scenes are the pessimistic case, as invariant 5's note warns.
  The theoretical risk was a **false split** (a true variant near 60 pushed
  over by drift); with the worst real pair at 34 there are 26 bits of
  headroom, so it does not arise. Re-run both ways after any decode change;
  `SEQUIN_FULL_DECODE=1` remains the rollback.
  ⚠️ One known sensitivity: border-trim is not stable across
  reconstructions. On one photo the trim removed 32.3% of area scaled vs
  24.2% full, moving `hash_cropped` by 90 bits. Harmless — that photo's
  `hash_full` is byte-identical on both paths (0 bits) and grouping takes
  `min(full, cropped)`, so the full-frame hash carries the match — but if
  border-trim logic is ever revisited, this is where it shows.

Visual system lives in `style.css` per DESIGN.md ("The Light Table": chroma-0
surfaces, macOS light/dark via `prefers-color-scheme`, honey-gold accent
≤10%, mono-for-data).

v2 ideas (do not start unless asked): CLIP-embedding outfit clustering via
`ort` (color histograms were tested and fail — they latch onto the backdrop,
not the dress), HEIC support, watch folders.
