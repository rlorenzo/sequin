#!/bin/sh
#
# Rebuild the app icon (icon.png + icon.icns) from the SVG masters in
# crates/sequin-app/assets/icon-src/. The mark and its rules are DESIGN.md
# "6. App Icon"; this script only rasterises them.
#
# Two masters are drawn by hand rather than one downscaled, because the detail
# master's four discs turn to mud below ~64px:
#
#   icon-detail.svg   4 discs, hairline separations  ->  128pt and up
#   icon-small.svg    3 discs, fat separations       ->  16pt and 32pt
#
# Requires librsvg for SVG rasterisation:  brew install librsvg
#
# Usage:
#   ./scripts/make_icon.sh            rebuild icon.icns and icon.png
#   ./scripts/make_icon.sh --check    verify the committed icons match the
#                                     masters; writes nothing, exits 1 on drift
set -eu

die() { echo "error: $*" >&2; exit 1; }

check=
case "${1-}" in
  --check) check=1 ;;
  "") ;;
  *) die "unknown argument: $1 (expected --check or nothing)" ;;
esac

root=$(cd "$(dirname "$0")/.." && pwd)
assets="$root/crates/sequin-app/assets"
src="$assets/icon-src"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for tool in rsvg-convert iconutil; do
  command -v "$tool" >/dev/null 2>&1 ||
    die "$tool not found. rsvg-convert comes from \`brew install librsvg\`; iconutil ships with macOS."
done

# The two masters draw different discs but share one body path. Nothing else
# enforces that -- DESIGN.md's Two-Master Rule is prose -- so check it here,
# where a redraw of one master has to pass through anyway.
for master in detail small; do
  grep -o 'd="M[^"]*"' "$src/icon-$master.svg" > "$work/body-$master"
done
cmp -s "$work/body-detail" "$work/body-small" ||
  die "the masters' body paths differ; the squircle would change between 32pt and 128pt"

iconset="$work/sequin.iconset"
mkdir -p "$iconset"

render() { # <svg> <px> <out>
  rsvg-convert -w "$2" -h "$2" "$1" -o "$3"
}

# The iconset has 10 slots but only 7 distinct images: each @2x is its 1x
# successor at the same pixel size (16@2x == 32@1x, and so on up the ladder).
# Render the 7, copy the 3 repeats.
render "$src/icon-small.svg"    16 "$iconset/icon_16x16.png"
render "$src/icon-small.svg"    32 "$iconset/icon_16x16@2x.png"
render "$src/icon-small.svg"    64 "$iconset/icon_32x32@2x.png"
render "$src/icon-detail.svg"  128 "$iconset/icon_128x128.png"
render "$src/icon-detail.svg"  256 "$iconset/icon_128x128@2x.png"
render "$src/icon-detail.svg"  512 "$iconset/icon_256x256@2x.png"
render "$src/icon-detail.svg" 1024 "$iconset/icon_512x512@2x.png"

cp "$iconset/icon_16x16@2x.png"   "$iconset/icon_32x32.png"
cp "$iconset/icon_128x128@2x.png" "$iconset/icon_256x256.png"
cp "$iconset/icon_256x256@2x.png" "$iconset/icon_512x512.png"

iconutil -c icns "$iconset" -o "$work/icon.icns"
cp "$iconset/icon_512x512@2x.png" "$work/icon.png"

if [ -n "$check" ]; then
  # Rasterisation is reproducible for a given librsvg/cairo, so this catches
  # the real failure -- a master edited without a rebuild. It is a local and
  # release-time check, deliberately not a CI gate: a runner on a different
  # cairo can differ in antialiasing without anything being wrong.
  for f in icon.icns icon.png; do
    cmp -s "$work/$f" "$assets/$f" ||
      die "$f is stale; re-run ./scripts/make_icon.sh and commit the result"
  done
  echo "icon.icns and icon.png match the masters"
  exit 0
fi

cp "$work/icon.icns" "$work/icon.png" "$assets/"
echo "wrote $assets/icon.icns"
echo "wrote $assets/icon.png"
