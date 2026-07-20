# Releasing Sequin

Packaging a distributable macOS build. Steps that need Apple credentials are
marked **(Apple Developer ID required)** — they can only be run by the
maintainer.

## Prerequisites

- macOS 11+ with Xcode command-line tools (`xcode-select --install`).
- The Dioxus CLI: `cargo binstall dioxus-cli` (or `cargo install dioxus-cli`).
- Bundle metadata lives in `crates/sequin-app/Dioxus.toml`; the app icon is
  `crates/sequin-app/assets/icon.icns` (regenerate from `icon.png` with
  `iconutil` — see [Regenerating the icon](#regenerating-the-icon)).

The order matters: **the app must be signed before the DMG is created**, and
the DMG is notarized last. `dx bundle` builds the DMG from the `.app` at
bundle time, so a DMG built alongside an unsigned app embeds the unsigned
copy — notarization would then reject it. So build only the `.app` first, sign
it, and wrap the *signed* app in the DMG afterward.

## 1. Build the app bundle (unsigned)

Run from the **workspace root** (running from the crate directory panics —
dx resolves the workspace from the top). Build only the `.app` here — the DMG
comes after signing:

```sh
dx bundle --package sequin-app --package-types macos
```

Artifact: `target/dx/sequin-app/bundle/macos/macos/Sequin.app`. It runs
locally, but Gatekeeper blocks it on other machines (right-click → Open is the
manual bypass) until it's signed and notarized.

## 2. Sign the app **(Apple Developer ID required)**

You need a "Developer ID Application" certificate in your login keychain
(from the Apple Developer portal). Sign nested code first, then the outer
bundle — Apple has deprecated `--deep` for production signing because it can
skip items it doesn't recognize and sign nested code in the wrong order:

```sh
APP=target/dx/sequin-app/bundle/macos/macos/Sequin.app
IDENTITY="Developer ID Application: Your Name (TEAMID)"

# Inner Mach-O first (dylibs, frameworks), if any appear as the bundler evolves.
find "$APP/Contents" \( -name '*.dylib' -o -name '*.framework' \) -print0 \
  | xargs -0 -I{} codesign --force --options runtime --timestamp --sign "$IDENTITY" {}

# Then the app itself, last.
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"
```

`hardened_runtime = true` is already set in `Dioxus.toml`, which notarization
requires. If the app is ever sandboxed or needs extra entitlements, add them
via `macos.entitlements` in `Dioxus.toml` and pass `--entitlements` here.

## 3. Package the signed app into a DMG

Build the disk image from the now-signed `.app` (Homebrew `create-dmg`, or
`hdiutil`):

```sh
create-dmg --volname "Sequin" --app-drop-link 480 170 \
  "target/dx/Sequin_0.1.0_aarch64.dmg" "$APP"
# or, minimal: hdiutil create -volname Sequin -srcfolder "$APP" -ov -format UDZO \
#   "target/dx/Sequin_0.1.0_aarch64.dmg"
```

## 4. Notarize the DMG and staple **(Apple Developer ID required)**

Store an app-specific password once (from appleid.apple.com):

```sh
xcrun notarytool store-credentials sequin-notary \
  --apple-id "you@example.com" --team-id "TEAMID" --password "app-specific-pw"
```

Submit the DMG, wait, then staple the ticket into it:

```sh
DMG=target/dx/Sequin_0.1.0_aarch64.dmg
xcrun notarytool submit "$DMG" --keychain-profile sequin-notary --wait
xcrun stapler staple "$DMG"
```

Verify:

```sh
spctl -a -vvv -t install "$APP"
xcrun stapler validate "$DMG"
```

## 4. Smoke test before shipping

On a real delivery (a **copy** — in-place mode writes real EXIF):

1. Open the folder, arrange the shoot, **Write timestamps…** (copy mode).
2. Import `sequin-output/` into Apple Photos.
3. Confirm the timeline order matches the arrangement, and grouped variants
   sit together.
4. Spot-check with `exiftool`:
   ```sh
   exiftool -DateTimeOriginal -csv sequin-output/*.jpg
   ```

## Regenerating the icon

The icon is generated from a script (kept out of the repo; see the
`make_icon.py` used during M5). To rebuild the `.icns` from a 1024×1024
`icon.png`:

```sh
mkdir sequin.iconset
for sz in 16 32 128 256; do
  sips -z $sz $sz         icon.png --out sequin.iconset/icon_${sz}x${sz}.png
  sips -z $((sz*2)) $((sz*2)) icon.png --out sequin.iconset/icon_${sz}x${sz}@2x.png
done
sips -z 512 512 icon.png --out sequin.iconset/icon_512x512.png
cp icon.png sequin.iconset/icon_512x512@2x.png   # already 1024×1024
iconutil -c icns sequin.iconset -o crates/sequin-app/assets/icon.icns
```

## Mac App Store (later, optional)

Per the Maccy model: the source stays free and buildable. A convenience build
may be offered on the MAS for users who'd rather pay than build. That path
uses a different certificate ("Apple Distribution") and an App Store Connect
record, and is out of scope until traction warrants it.
