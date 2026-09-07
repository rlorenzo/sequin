<div align="center">
  <img src="crates/sequin-app/assets/icon.png" width="128" alt="Sequin app icon" />
  <h1>Sequin</h1>
  <strong>Put your photoshoot back in order.</strong>
</div>

---

Studio photo deliveries arrive with random UUID filenames and *identical* EXIF
capture times, so Apple Photos — which sorts strictly by capture time — shows
the whole shoot in arbitrary order, with every styled variant of a shot
scattered across the timeline. Photos can't fix this after import: its batch
date adjustment only applies a uniform shift.

Sequin fixes it before import:

1. **Groups** the styled variants of each shot (B&W conversions, text
   overlays, background swaps, borders) using perceptual hashing.
2. Lets you **drag groups and photos into shoot order** on a photo-first
   light table — with full keyboard control and undo.
3. **Writes sequential EXIF capture times** so the batch imports in exactly
   the order you arranged.

No existing tool combines similarity grouping, drag-to-reorder, and sequential
date writing in one step (the closest is a ~$60 two-app round trip with no
visual grouping).

## Status

The core flow is complete: **group → arrange → write.**

- **Grouping** — custom pHash (64×64 DCT, top-left 16×16, median threshold),
  computed on the full frame and a border-trimmed copy, clustered with
  union-find. Validated on a real 62-photo delivery: 34 groups, zero false
  merges.
- **Arranging** — drag or keyboard reordering of groups and photos, merge and
  split, multi-select, 100-deep undo/redo, and an autosaved `arrangement.json`
  sidecar that resumes your session and doubles as the CLI interchange format.
- **Writing** — sequential timestamps written to `DateTimeOriginal`,
  `CreateDate`, and `ModifyDate`, **copy-safe by default** (stamps copies in a
  `sequin-output/` folder; originals untouched), with dry-run preview,
  per-file failure reporting, and read-back verification.

Packaging and notarization (M5) are in progress; see [PLAN.md](PLAN.md).

## Install / run

macOS 11 (Big Sur) or newer.

```sh
git clone https://github.com/rlorenzo/sequin
cd sequin
cargo run -p sequin-app          # or: dx serve -p sequin-app (hot reload)
```

Open a folder of photos, arrange them, and click **Write timestamps…**. Import
the `sequin-output/` folder into Apple Photos.

### Keyboard

| Key | Action |
|-----|--------|
| Arrows | Move selection between photos / groups |
| ⌘ + Arrows | Move the selected photo or group |
| ⇧ / ⌘ + click, ⇧ + Arrows | Extend the selection |
| **M** | Merge selected groups |
| **S** | Split the selected photo into its own group |
| ⌘Z / ⇧⌘Z | Undo / redo |
| Esc | Clear selection |

## CLI

The `sequin` CLI exposes the same core engine, and is the golden-test harness:

```sh
cargo build --release
./target/release/sequin group ~/Downloads/my-shoot > arrangement.json
# edit arrangement.json to reorder, or produce it from the GUI, then:
./target/release/sequin apply arrangement.json "2026-07-18 10:00" --dry-run
./target/release/sequin apply arrangement.json "2026-07-18 10:00"   # copies to sequin-output/
./target/release/sequin apply arrangement.json "2026-07-18 10:00" --in-place
```

## Develop

```sh
cargo test                 # or: cargo nextest run
cargo clippy --workspace --all-targets -- -D warnings
```

Quality gates (fmt, clippy, tests, typos, cargo-deny) run in CI and in the
`.githooks/pre-commit` hook — enable it once per clone with
`git config core.hooksPath .githooks`. The workspace layout, validated
algorithm invariants, and architecture notes live in
[CLAUDE.md](CLAUDE.md); the roadmap is in [PLAN.md](PLAN.md); packaging and
notarization steps are in [RELEASE.md](RELEASE.md).

**Privacy:** no photos are ever committed to this repository — `.gitignore`
blocks all image formats, and the golden-test fixture contains filenames only.

## License

MIT — free and open source. Following the [Maccy](https://maccy.app) model, a
signed, notarized convenience build may be offered on the Mac App Store later
to support development; the source stays free and buildable by anyone.
