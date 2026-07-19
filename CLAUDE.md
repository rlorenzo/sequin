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

Golden test (run whenever touching hashing/grouping code): group the local
test delivery and compare against `fixtures/expected_groups_archive1-2.json`
(sorted filename sets must match EXACTLY — 34 groups from 62 photos):

```sh
./target/release/sequin group ~/Downloads/Archive1-2   # photos live only on Rex's Mac
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
4. **Cluster threshold 60/256** (union-find over pairs). Real-photo band:
   true variants ≤ 60, nearest false pair ≥ 102.
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

## Current state / next work

M1 (core pipeline) is DONE and validated. Next is M2 in PLAN.md: thumbnail
grid grouped by cluster, then M3 drag-to-reorder (the core UX), then M4 time
assignment + EXIF write flow. The Dioxus shell in `sequin-app/src/main.rs`
currently only picks a folder and lists group filenames — replace its group
list with the thumbnail grid, keeping the spawn_blocking pattern.

## v2 ideas (do not start unless asked)

CLIP-embedding outfit clustering via `ort` (color histograms were tested and
fail — they latch onto the backdrop, not the dress), HEIC support, watch
folders.
