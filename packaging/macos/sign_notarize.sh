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
#   SKIP_BUILD=1    reuse an existing .app instead of running `dx bundle`
#   SKIP_NOTARIZE=1 sign and package only (useful when iterating on signing)
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

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

# Apple/Rust spell the same silicon differently; the DMG name uses the Rust
# target triple's word, matching the artifact shipped in July.
case "$(uname -m)" in
  arm64) ARCH=aarch64 ;;
  x86_64) ARCH=x86_64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

APP_PATH="${APP_PATH:-target/dx/sequin-app/bundle/macos/macos/Sequin.app}"
DMG_PATH="${DMG_PATH:-target/dx/Sequin_${VERSION}_${ARCH}.dmg}"

# 1. Build the .app ONLY. The DMG is built after signing, further down:
# `dx bundle --package-types dmg` would wrap the unsigned app, and Apple
# rejects a DMG whose payload is not already signed.
if [ -z "${SKIP_BUILD:-}" ]; then
  step "Building $APP_PATH"
  dx bundle --package sequin-app --package-types macos
fi
[ -d "$APP_PATH" ] || die "app bundle not found: $APP_PATH"

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
