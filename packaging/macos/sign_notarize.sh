#!/usr/bin/env bash
#
# Sign, package, notarize and staple the macOS build.
#
# One script for both paths -- a local release from RELEASE.md and the CI job
# in .github/workflows/release.yml -- so what CI ships is the same thing that
# was validated by hand. The two differ only in how they authenticate to
# Apple's notary service.
#
# Required:
#   SIGN_IDENTITY   "Developer ID Application: Your Name (TEAMID)"
#
# Notarization, exactly one of:
#   NOTARY_PROFILE  keychain profile name (local; e.g. sequin-notary)
#   NOTARY_KEY + NOTARY_KEY_ID + NOTARY_ISSUER
#                   App Store Connect API key (CI; a runner has no keychain
#                   profile, so it authenticates with the .p8 directly)
#
# Optional:
#   APP_PATH        default target/dx/sequin-app/bundle/macos/macos/Sequin.app
#   DMG_PATH        default target/dx/Sequin_<version>_<arch>.dmg
#   UNIVERSAL=1     build arm64 + x86_64 and lipo them into one binary
#   SKIP_BUILD=1    reuse an existing .app instead of running `dx bundle`
#   SKIP_NOTARIZE=1 sign and package only (useful when iterating on signing)
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

# Fuse every Mach-O in the bundle $2 with its counterpart in bundle $1, in
# place. Not just the main executable: the bundle holds one today, but a
# vendored dylib or a helper tool would otherwise ship thin inside an app
# advertised as universal, and this keeps working when that happens instead
# of blocking a release. `lipo -info` is the Mach-O test -- it fails on
# scripts and other executable data, which are architecture-neutral anyway.
fuse_slices() {
  local from="$1" into="$2" thin rel
  while IFS= read -r -d '' thin; do
    # `lipo -info` IS the Mach-O test, so no permission or name filter is
    # needed -- and none is wanted. Filtering on -perm -a+x would skip a
    # dylib shipped 0644, which the signing pass (matching on *.dylib)
    # still signs: the bundle would then be signed everywhere but fused
    # only where the executable bit happened to be set, and ship thin
    # libraries inside an app advertised as universal.
    lipo -info "$thin" >/dev/null 2>&1 || continue
    rel="${thin#"$into"/}"
    [ -f "$from/$rel" ] || die "no arm64 counterpart for $rel"
    echo "  fusing: $rel"
    lipo -create "$from/$rel" "$thin" -output "$thin.universal"
    mv "$thin.universal" "$thin"
    # Fusing silently producing one slice would defeat the point.
    lipo "$thin" -verify_arch arm64 x86_64 \
      || die "$rel is not universal after fusing: $(lipo -archs "$thin")"
  done < <(find "$into/Contents" -type f -print0)
}

cd "$(dirname "${BASH_SOURCE[0]}")/../.."   # workspace root; dx resolves from here

: "${SIGN_IDENTITY:?set SIGN_IDENTITY to your Developer ID Application identity}"

# Exactly one notarization method, checked up front: discovering this after a
# five-minute build and a signing pass is a waste of everyone's time.
if [ -z "${SKIP_NOTARIZE:-}" ]; then
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    [ -n "${NOTARY_KEY:-}" ] && die "set NOTARY_PROFILE or NOTARY_KEY, not both"
    notary_auth=(--keychain-profile "$NOTARY_PROFILE")
  elif [ -n "${NOTARY_KEY:-}" ]; then
    : "${NOTARY_KEY_ID:?NOTARY_KEY needs NOTARY_KEY_ID}"
    : "${NOTARY_ISSUER:?NOTARY_KEY needs NOTARY_ISSUER}"
    [ -f "$NOTARY_KEY" ] || die "NOTARY_KEY file not found: $NOTARY_KEY"
    notary_auth=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")
  else
    die "set NOTARY_PROFILE (local) or NOTARY_KEY/_ID/_ISSUER (CI), or SKIP_NOTARIZE=1"
  fi
fi

VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
           | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
[ -n "$VERSION" ] || die "could not read version from [workspace.package] in Cargo.toml"

APP_PATH="${APP_PATH:-target/dx/sequin-app/bundle/macos/macos/Sequin.app}"

# 1. Build the .app ONLY. The DMG is built after signing, further down:
# `dx bundle --package-types dmg` would wrap the unsigned app, and Apple
# rejects a DMG whose payload is not already signed.
if [ -z "${SKIP_BUILD:-}" ]; then
  if [ -n "${UNIVERSAL:-}" ]; then
    # dx has --target but no universal mode, and it writes every target to
    # the SAME bundle path -- so the second build clobbers the first. Build
    # arm64, keep a copy, then let the x86_64 build take over $APP_PATH and
    # fuse the saved slices back into it. lipo has to run before codesign:
    # fattening a signed binary invalidates its signature, exactly the way
    # wrapping a signed app in a DMG built from an unsigned one does.
    step "Building universal (arm64 + x86_64)"
    stage="$(mktemp -d)"
    trap 'rm -rf "$stage"' EXIT

    dx bundle --package sequin-app --package-types macos --target aarch64-apple-darwin
    [ -d "$APP_PATH" ] || die "arm64 bundle not found: $APP_PATH"
    ditto "$APP_PATH" "$stage/arm64.app"   # ditto, not cp -R: bundle-aware

    dx bundle --package sequin-app --package-types macos --target x86_64-apple-darwin
    [ -d "$APP_PATH" ] || die "x86_64 bundle not found: $APP_PATH"

    step "Fusing slices"
    fuse_slices "$stage/arm64.app" "$APP_PATH"
  else
    step "Building $APP_PATH"
    dx bundle --package sequin-app --package-types macos
  fi
fi
[ -d "$APP_PATH" ] || die "app bundle not found: $APP_PATH"

# Name the DMG for what the binary actually CONTAINS rather than for the host
# or for the flag that asked for it: SKIP_BUILD=1 reuses whatever bundle is on
# disk, and a fat app shipped as "_aarch64" is a lie the user cannot see.
# Apple and Rust spell the same silicon differently; the name uses Rust's word.
# `-verify_arch` is an exact match -- a substring test on `lipo -archs` would
# also accept arm64e, which is the opposite of what this check is for.
exe="$(plutil -extract CFBundleExecutable raw -o - "$APP_PATH/Contents/Info.plist")" \
  || die "could not read CFBundleExecutable from $APP_PATH/Contents/Info.plist"
main_exe="$APP_PATH/Contents/MacOS/$exe"
archs="$(lipo -archs "$main_exe")"
if lipo "$main_exe" -verify_arch arm64 x86_64; then
  ARCH=universal
elif lipo "$main_exe" -verify_arch arm64; then
  ARCH=aarch64
elif lipo "$main_exe" -verify_arch x86_64; then
  ARCH=x86_64
else
  die "unexpected architectures in $exe: $archs"
fi
echo "  slices: $archs"

# UNIVERSAL promised both. Check the build kept the promise -- a silently
# single-arch release is worse than no universal build at all.
[ -z "${UNIVERSAL:-}" ] || [ "$ARCH" = universal ] \
  || die "UNIVERSAL=1 but the app is $archs -- refusing to ship a thin binary"

DMG_PATH="${DMG_PATH:-target/dx/Sequin_${VERSION}_${ARCH}.dmg}"

# 2. Sign inner Mach-O first, then the outer bundle. Apple deprecated
# --deep for production signing because it can sign nested code in the
# wrong order and skip items it does not recognize. `find -depth` is what
# keeps the order right here: without it find yields a *.framework before
# the *.dylib inside it, and signing the dylib second breaks the seal on
# the framework that was already signed.
step "Signing $APP_PATH"
sign_args=(--force --options runtime --timestamp --sign "$SIGN_IDENTITY")

while IFS= read -r -d '' nested; do
  echo "  nested: ${nested#"$APP_PATH"/}"
  codesign "${sign_args[@]}" "$nested"
done < <(find "$APP_PATH/Contents" -depth \( -name '*.dylib' -o -name '*.framework' \) -print0)

codesign "${sign_args[@]}" "$APP_PATH"
codesign --verify --strict --verbose=2 "$APP_PATH"

# 3. Package the SIGNED app. create-dmg gives the styled window with the
# Applications drop link; hdiutil is the dependency-free fallback and
# produces a perfectly valid, notarizable image.
step "Packaging $DMG_PATH"
mkdir -p "$(dirname "$DMG_PATH")"
rm -f "$DMG_PATH"
if command -v create-dmg >/dev/null 2>&1; then
  create-dmg --volname "Sequin" --app-drop-link 480 170 "$DMG_PATH" "$APP_PATH"
else
  echo "  create-dmg not found; falling back to hdiutil"
  hdiutil create -volname Sequin -srcfolder "$APP_PATH" -ov -format UDZO "$DMG_PATH"
fi

if [ -n "${SKIP_NOTARIZE:-}" ]; then
  step "SKIP_NOTARIZE set -- signed and packaged, NOT notarized"
  echo "$DMG_PATH"
  exit 0
fi

# 4. Notarize the DMG and staple the ticket into it, so Gatekeeper clears it
# without a network round trip on the user's machine.
step "Notarizing $DMG_PATH"
xcrun notarytool submit "$DMG_PATH" "${notary_auth[@]}" --wait

step "Stapling"
xcrun stapler staple "$DMG_PATH"

step "Verifying"
spctl -a -vvv -t exec "$APP_PATH"
xcrun stapler validate "$DMG_PATH"

printf '\nDone: %s\n' "$DMG_PATH"
