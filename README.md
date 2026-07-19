# Sequin

**Put your photoshoot back in order.** Sequin groups the styled variants of
each shot in a studio photo delivery (B&W conversions, text overlays,
background swaps, borders), lets you arrange groups in shoot order, and writes
sequential EXIF capture times — so the batch imports into Apple Photos in the
right order on the timeline.

Studio galleries usually download with random filenames and identical
timestamps. Apple Photos sorts strictly by capture time and can't fix this
after import. Sequin fixes it before.

## Status

Early development. The core pipeline (perceptual-hash grouping, timestamp
assignment, EXIF writing) is implemented and tested; the Dioxus GUI is a
minimal shell. See [PLAN.md](PLAN.md) for the roadmap.

## Try the CLI

```sh
cargo run -p sequin-cli --bin sequin -- group ~/Downloads/my-shoot > arrangement.json
# edit arrangement.json to reorder groups/photos, then:
cargo run -p sequin-cli --bin sequin -- apply arrangement.json "2026-07-18 10:00" --dry-run
```

## Run the app (macOS)

```sh
cargo run -p sequin-app
# or with hot reload: dx serve -p sequin-app
```

## Develop

```sh
cargo test          # core tests (no GUI deps needed)
cargo build         # builds core + cli (app excluded from default members)
```

## License

MIT — free and open source. A signed, notarized convenience build may be
offered on the Mac App Store in the future to support development.
