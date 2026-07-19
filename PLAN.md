# Sequin — Implementation Plan

Group styled variants of the same photo, arrange them in shoot order, and write
sequential EXIF capture times so Apple Photos shows them correctly in the
timeline. macOS desktop app built with [Dioxus](https://dioxuslabs.com), core
logic in a headless Rust crate.

## Why this exists (validated 2026-07-19)

Studio deliveries arrive with randomized UUID filenames and *identical* EXIF
capture times (the test batch: all 62 photos stamped `2026-07-18 00:00:00`).
Apple Photos sorts the timeline strictly by `DateTimeOriginal` and its batch
date adjustment can only apply a uniform shift — so ordering must be fixed
*before* import by rewriting EXIF. No existing app combines similarity
grouping + drag-reorder + sequential date writing (closest: A Better Finder
Rename + A Better Finder Attributes, ~$60, two-app round trip, no visual
grouping).

## Validated algorithm decisions (do not re-derive)

- **pHash: 64×64 grayscale → 2D DCT-II → top-left 16×16 coeffs → bit =
  coeff > median (256 bits)**, computed on the full frame AND a
  border-trimmed copy; pairwise distance = min of the two. Threshold **≤ 60**.
  On the real 62-photo batch this produced 34 groups with zero false merges;
  nearest false pair was at distance 102 (wide margin). Fixture:
  `fixtures/expected_groups_archive1-2.json` (filenames only, no image data).
- Variants successfully matched: B&W conversions, text overlays, background
  swaps, small borders. **Not matched (by design/limitation): alternate
  crops and collage composites** — the UI must support dragging strays into a
  group manually.
- **Outfit/costume grouping cannot be done with hashes or color histograms**
  (tested: color features latch onto the backdrop, not the dress). v2 feature:
  CLIP-style embeddings via `ort` or `candle`, clustered into outfit buckets;
  classify at the *group* level so B&W variants inherit from their color
  siblings. v1: manual outfit buckets in the UI.
- **Timestamps:** groups 60s apart, photos within a group 10s apart (defaults
  in `timeline::Spacing`). Write `DateTimeOriginal`, `CreateDate`, and
  `ModifyDate` together via `little_exif`.

## Workspace layout

- `crates/sequin-core` — headless: hashing, clustering, timeline, EXIF write.
  Fully tested without a GUI. **Keep it GUI-free.**
- `crates/sequin-cli` — thin CLI over the core (`sequin group`, `sequin
  apply`). Useful for testing and scripting; also the reference for how the
  GUI should call the core.
- `crates/sequin-app` — Dioxus desktop app. Excluded from
  `default-members` so `cargo build`/`cargo test` work without webview deps.
  Build it with `dx serve -p sequin-app` / `cargo run -p sequin-app` on macOS.

## Milestones

### M1 — Core pipeline (DONE, verified against real data)
Hashing, border-trim, union-find clustering, timestamp assignment, EXIF
round-trip. `cargo test` covers: bordered-variant grouping (synthetic),
timestamp spacing, EXIF write/read round-trip. CLI reproduces the validated
grouping of the test batch byte-for-byte against the fixture.

### M2 — Thumbnail grid (GUI)
- Generate thumbnails (~256px JPEG) into an app cache dir
  (`dirs::cache_dir()/sequin/<hash-of-source-path>/`), parallel via rayon;
  show progress while hashing (channel from `spawn_blocking` → Signal).
- Render groups as horizontal rows of thumbnails (like the contact sheet that
  validated the algorithm). Data URL or custom asset handler for images —
  Dioxus desktop can serve local files via `use_asset_handler`.
- Show group badges: photo count, border/B&W indicators (`border_fraction`,
  mean saturation are already computed or trivial to add).

### M3 — Arrangement editing (the core UX)
- Drag to reorder groups (vertical); drag to reorder photos within a group
  (horizontal); drag a photo *between* groups (fixes crop/collage strays).
  HTML5 drag-and-drop works in the webview; `dioxus-sortable` exists but
  hand-rolling with `ondragstart`/`ondrop` is fine and dependency-free.
- Multi-select (cmd-click) + "merge groups" / "split photo out" actions.
- Keyboard: arrows move selection, cmd+arrows move the selected group.
- Persist the arrangement to a sidecar JSON (same schema as `sequin group`
  output) so a session can be resumed; this file is also the CLI interchange.

### M4 — Time assignment + EXIF write
- Toolbar: date/time picker for shoot start (default: file mtime date at
  10:00), spacing controls with the validated defaults, live preview of the
  first/last computed timestamp.
- "Write timestamps" button → confirm dialog listing count → progress →
  per-file failure report. Always offer dry-run preview first.
- Option (default ON): copy files to a `sequin-output/` folder instead of
  writing in place, preserving originals.
- Verify-after-write: read back `DateTimeOriginal` on a sample and confirm.

### M5 — Polish & release
- App icon, DMG packaging (`dx bundle` / `cargo-bundle`), notarization.
- README with the Maccy-model pitch: free & open source (MIT), paid
  convenience build on the Mac App Store later if traction warrants.
- Manual smoke test: full flow on a real delivery → import to Apple Photos →
  timeline order correct.

### v2 candidates
- CLIP outfit clustering (`ort` + quantized ViT-B/32, ~30–150 MB model).
- Watch folder / auto-detect new deliveries.
- HEIC support (`little_exif` roadmap; fall back to bundled `exiftool` if
  needed).
- Localization of the timestamp-offset UI (time zones are intentionally
  ignored: `DateTimeOriginal` is naive local time, which is what Photos uses).

## Testing strategy

- Unit tests in core (already passing): synthetic bordered-variant grouping,
  spacing math, EXIF round-trip.
- **Golden test against real data:** `sequin group <folder>` on the
  Archive1-2 delivery must reproduce `fixtures/expected_groups_archive1-2.json`
  exactly (compare sorted filename sets). The photos themselves are NOT in the
  repo (gitignored; privacy) — this test is `#[ignore]`/CLI-driven and run
  locally: `SEQUIN_TEST_DIR=~/Downloads/Archive1-2 cargo test -- --ignored`.
- After M4: end-to-end dry-run on a copy of the folder, then verify EXIF with
  `exiftool -DateTimeOriginal -csv` if available.

## Gotchas discovered so far

- `little_exif` tag variants take EXIF-format strings (`YYYY:MM:DD HH:MM:SS`);
  read-back values may be NUL-padded — trim before comparing.
- The two backdrop-only texture images in the test delivery cluster as
  singletons; consider a low-variance detector to auto-bucket them as
  "extras" (they shouldn't get timeline slots by default).
- **`image_hasher` was tried and rejected**: its `.preproc_dct()` uses a
  32×32 DCT with mean thresholding, which collapsed 51 of the 62 real photos
  into ONE cluster (portraits share too much coarse structure). The custom
  pHash in `hashing.rs` (64×64 DCT, top-left 16×16, median threshold — same
  construction as Python imagehash) reproduces the validated grouping
  exactly. Don't swap hash implementations without re-running the golden
  test against the fixture.
- Synthetic images are a worst case for the border-trim path (hard border +
  high-frequency texture ⇒ variant distance ~96); real bordered variants
  matched well inside 60. If a future delivery has heavy decorative frames
  that don't auto-group, improving `trim_border` is the lever.
- Dioxus 0.7 API churn: pin the minor version; `dioxus::launch` +
  `use_signal`/`Signal` idioms as used in `sequin-app/src/main.rs`.
