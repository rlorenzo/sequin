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

## Releasing from a tag (CI)

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds, signs,
notarizes, staples, verifies, and attaches the `.dmg` to a **draft** GitHub
release for you to review and publish:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

It signs on a GitHub-hosted macOS runner using six secrets held on the
`release` **environment** — Settings → Environments → `release` → *Environment
secrets*. Create that environment first; see [Protect the `release`
environment](#protect-the-release-environment) for why the scope matters:

| Secret | Value |
|---|---|
| `MACOS_CERT_P12` | Developer ID Application cert **and private key**, exported as `.p12`, then base64-encoded |
| `MACOS_CERT_PASSWORD` | the password set on that `.p12` export |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `NOTARY_KEY_P8` | contents of the App Store Connect API key `.p8` |
| `NOTARY_KEY_ID` | that key's Key ID |
| `NOTARY_ISSUER` | the App Store Connect issuer UUID |

Export the certificate from Keychain Access (select the **identity**, not just
the certificate, so the private key travels with it), then:

```sh
base64 -i Certificates.p12 | pbcopy    # paste into MACOS_CERT_P12
```

The notary key is separate from the local `sequin-notary` keychain profile: a
runner has no keychain to hold a profile, so it authenticates to Apple with the
API key directly. Create one at App Store Connect → Users and Access → Keys
(role: Developer). Apple lets you download the `.p8` **once**.

Set **all six or none**. Half-configured fails the run and names what is
missing, rather than quietly handing back an unsigned artifact that looks like
a release.

### Protect the `release` environment

The signing job runs in a GitHub environment named `release`, which is where
the credential boundary lives.

Scope is what makes that boundary real. A **repository** secret (Settings →
Secrets and variables → Actions) is readable by any job in any workflow in the
repo, so keeping the signing values there would make `environment: release`
decorative: a collaborator could add a job that reads the Developer ID key
directly — no environment, no approval, neither preflight check. Store all six
on the environment instead. If they already exist at repository scope, re-add
them under the environment and then **delete the repository-level copies**;
delete any organization-level copies this repo can reach as well, since those
bypass the environment the same way.

Anyone who can push a `v*` tag or dispatch the workflow can point the Developer
ID key at whatever code that revision contains, so protect the environment
before the first real tag: add yourself as a **required reviewer**, and
restrict deployment branches/tags to `v*` and `main`. A run then waits for
approval before any key material is materialized.

Two further guards run automatically on tag pushes:

- **The tagged commit must be an ancestor of `origin/main`**, so a tag pushed
  at an unreviewed commit is refused before signing.
- **The tag must match `[workspace.package] version`** in `Cargo.toml`, so a
  `v0.2.0` release can never ship a `Sequin_0.1.0` DMG.

Neither applies to `workflow_dispatch` (that is the point — it exists to
rehearse from a branch), so the environment's reviewer rule is what guards
that path.

The runner imports the cert into a throwaway keychain that dies with the job,
and wipes the `.p8` and `.p12` on any exit, including a failed signing run.
Without the secrets — a fork, or a dispatch before they are configured — the
job still builds, but produces a `Sequin-UNSIGNED.dmg` that Gatekeeper will
block on other machines — and the draft-release job refuses to attach it.

## Releasing locally

The same script CI runs, driven by the keychain profile instead of an API key:

```sh
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
NOTARY_PROFILE=sequin-notary \
  ./packaging/macos/sign_notarize.sh
```

It builds the `.app`, signs it, packages the **signed** app into
`target/dx/Sequin_<version>_<arch>.dmg`, notarizes, staples and verifies.
Useful switches: `SKIP_BUILD=1` to reuse an existing bundle, `SKIP_NOTARIZE=1`
to stop after packaging.

Steps 1–4 below are what that script automates, kept as the reference for
debugging a failure or signing by hand.

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
# -depth matters: without it a framework is signed before the dylib inside it,
# and signing the dylib second breaks the framework's seal.
find "$APP/Contents" -depth \( -name '*.dylib' -o -name '*.framework' \) -print0 \
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
spctl -a -vvv -t exec "$APP"
xcrun stapler validate "$DMG"
```

## 5. Smoke test before shipping

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
