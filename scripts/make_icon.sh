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
# macOS 26 wants a layered Liquid Glass icon instead, so this also rebuilds the
# Icon Composer document and compiles it:
#
#   icon-src/layers/*.svg  ->  Sequin.icon/Assets/*.png  ->  Assets.car
#
# Assets.car is committed rather than built at release time. actool is not
# reproducible -- it stamps a build timestamp and per-rendition UUIDs, so two
# compiles of identical input differ -- and DESIGN.md requires a human to check
# a redraw against all seven renditions first. So it is checked in like any
# other reviewed artwork, not regenerated per release. (The release runner is
# macos-14, which has no Xcode 26 and so no .icon support, but that is the
# lesser reason: bumping the runner would not make the catalogue reproducible.)
#
# Requires librsvg for SVG rasterisation:  brew install librsvg
# The Assets.car step additionally requires Xcode 26 (for actool).
#
# Usage:
#   ./scripts/make_icon.sh            rebuild icns, png, layer PNGs, Assets.car
#   ./scripts/make_icon.sh --check    verify the committed icns, png and layer
#                                     PNGs match the masters; writes nothing,
#                                     exits 1 on drift
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

# actool compiles the layered icon, and only a write run needs it. Probed here
# rather than at the point of use so a machine that cannot do the whole job
# says so before doing any of it.
if [ -z "$check" ]; then
  xcrun --find actool >/dev/null 2>&1 ||
    die "actool not found. The layered macOS 26 icon needs Xcode 26; --check works without it."
  # Existence is not enough: .icon documents need the Xcode 26 actool, and an
  # older one accepts the invocation and then fails deep in the compile, after
  # the rasterisation has already run. Check the version, not the binary.
  xcode_ver="$(xcodebuild -version 2>/dev/null | sed -n 's/^Xcode \([0-9][0-9]*\).*/\1/p')"
  [ -n "$xcode_ver" ] ||
    die "could not read the Xcode version. The layered icon needs a full Xcode 26 install (command line tools alone are not enough)."
  [ "$xcode_ver" -ge 26 ] ||
    die "Xcode $xcode_ver is too old for .icon documents; the layered macOS 26 icon needs Xcode 26 or newer. --check works without it."
fi

# The two masters draw different discs but share one body path. Nothing else
# enforces that -- DESIGN.md's Two-Master Rule is prose -- so check it here,
# where a redraw of one master has to pass through anyway.
for master in detail small; do
  grep -o 'd="M[^"]*"' "$src/icon-$master.svg" > "$work/body-$master"
done
cmp -s "$work/body-detail" "$work/body-small" ||
  die "the masters' body paths differ; the squircle would change between 32pt and 128pt"

# Same reasoning one level out: DESIGN.md's gold ramp is now written down in
# three files -- the detail master and the two layer masters -- and only prose
# pairs them, so a recolour of one could ship a flat icon and a layered icon
# that disagree. Colours are what a redraw actually changes; the geometry is
# spelled differently between the two constructions (the layers translate a
# shared disc) and does not compare mechanically.
grep -o 'fill="#[0-9A-F]\{6\}"' "$src/icon-detail.svg" | grep -v '#050505' | sort -u > "$work/fills-flat"
cat "$src"/layers/*.svg | grep -o 'fill="#[0-9A-F]\{6\}"' | sort -u > "$work/fills-layers"
cmp -s "$work/fills-flat" "$work/fills-layers" ||
  die "the flat and layered masters use different disc colours; DESIGN.md's ramp must be one ramp"

iconset="$work/sequin.iconset"
mkdir -p "$iconset"

render() { # <svg> <px> <out>
  rsvg-convert -w "$2" -h "$2" "$1" -o "$3"
}

# Digest of everything actool consumes, for the staged .icon document in $1.
# Assets.car itself cannot be compared -- actool stamps a build timestamp and
# per-rendition UUIDs, so two compiles of identical input differ -- but its
# INPUTS are reproducible, and a stale catalogue is exactly the case where they
# no longer match what was compiled. Sorted so the digest does not depend on
# glob order.
car_inputs() { # <staged-icon-doc>
  # The glob expands in sorted order, so the digest does not depend on the
  # order the filesystem happens to hand them back.
  {
    cat "$1/icon.json"
    for asset in "$1"/Assets/*; do cat "$asset"; done
  } | shasum -a 256 | cut -d' ' -f1
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

# The layered macOS 26 icon. Staged in $work alongside the icns, so a failure
# anywhere leaves the committed assets untouched and --check can compare
# against the tree without writing to it.
#
# The layer PNGs carry real transparency where the discs are cut apart. That
# matters more here than in the flat icon: a layered icon composites over
# whatever ground the system chooses per appearance, so a separation "drawn"
# by painting the ground colour would show up as a black scar in the tinted
# and clear renditions. The masters do the cutting with an SVG mask rather
# than a fill-rule; the why is in icon-src/layers/.
icon_doc="$assets/Sequin.icon"
staged="$work/Sequin.icon"
mkdir -p "$staged/Assets"
cp "$icon_doc/icon.json" "$staged/"
for svg in "$src"/layers/*.svg; do
  render "$svg" 1024 "$staged/Assets/$(basename "$svg" .svg).png"
done

if [ -n "$check" ]; then
  # Rasterisation is reproducible for a given librsvg/cairo, so this catches
  # the real failure -- a master edited without a rebuild. It is a local and
  # release-time check, deliberately not a CI gate: a runner on a different
  # cairo can differ in antialiasing without anything being wrong.
  for f in icon.icns icon.png; do
    cmp -s "$work/$f" "$assets/$f" ||
      die "$f is stale; re-run ./scripts/make_icon.sh and commit the result"
  done
  for png in "$staged"/Assets/*.png; do
    cmp -s "$png" "$icon_doc/Assets/$(basename "$png")" ||
      die "Sequin.icon/Assets/$(basename "$png") is stale; re-run ./scripts/make_icon.sh and commit the result"
  done
  # Assets.car is not compared byte for byte, but its inputs are: if the layer
  # PNGs or icon.json have moved on from whatever was last compiled, the
  # committed catalogue is stale and a release would ship the old icon.
  if [ -f "$assets/Assets.car.inputs" ]; then
    [ "$(car_inputs "$staged")" = "$(cat "$assets/Assets.car.inputs")" ] ||
      die "Assets.car is stale -- it was compiled from different layers or icon.json; re-run ./scripts/make_icon.sh on a machine with Xcode 26 and commit the result"
  else
    die "$assets/Assets.car.inputs is missing; re-run ./scripts/make_icon.sh to stamp what the catalogue was built from"
  fi
  # The catalogue is deliberately not compared: actool stamps a build timestamp
  # and per-rendition UUIDs, so two compiles of identical input differ by a few
  # hundred bytes and cmp would fail every time. The layer PNGs above are the
  # reproducible half, and they are the half a redraw actually changes.
  echo "icon.icns, icon.png, the layer PNGs and Assets.car's inputs all match the masters"
  exit 0
fi

carout="$work/car"
mkdir -p "$carout"
# --app-icon names the asset inside the catalogue, and Info.plist's
# CFBundleIconName has to match that name or macOS 26 falls back to the .icns
# without complaining. Both this script and sign_notarize.sh derive the one
# word from the .icon document's filename rather than spelling it out twice.
#
# actool writes its notices, warnings and errors to stdout, so they are left
# on stdout: silencing them would mean asking for diagnostics and then hiding
# the reason `set -e` aborted.
xcrun actool "$staged" --compile "$carout" \
  --output-format human-readable-text --notices --warnings --errors \
  --output-partial-info-plist "$carout/partial.plist" \
  --app-icon "$(basename "$icon_doc" .icon)" --include-all-app-icons \
  --enable-on-demand-resources NO --development-region en \
  --target-device mac --minimum-deployment-target 26.0 --platform macosx

[ -f "$carout/Assets.car" ] || die "actool produced no Assets.car"

# Everything is built; install it in one pass. actool also emits its own .icns
# from the .icon document -- ours is kept instead, because it has hand-drawn
# 16pt/32pt artwork that a render of the layered document does not.
cp "$work/icon.icns" "$work/icon.png" "$assets/"
cp "$staged"/Assets/*.png "$icon_doc/Assets/"
cp "$carout/Assets.car" "$assets/Assets.car"
car_inputs "$staged" > "$assets/Assets.car.inputs"
echo "wrote $assets/icon.icns"
echo "wrote $assets/icon.png"
for png in "$staged"/Assets/*.png; do
  echo "wrote $icon_doc/Assets/$(basename "$png")"
done
echo "wrote $assets/Assets.car"
echo "wrote $assets/Assets.car.inputs"
