# Releasing the desktop app

The bridge's release procedure, and how users verify both products' downloads,
are in [`deploy/README.md`](../deploy/README.md#releasing-the-bridge). What
changed in each release is recorded in [`CHANGELOG.md`](../CHANGELOG.md).

## Electron releases

The `Release the desktop app` GitHub workflow runs on `desktop-vX.Y.Z` tags.
The tag must match `desktop/package.json` and its lockfile. It builds native
macOS Apple Silicon (ARM64) and Linux ARM64/x86_64 apps. Intel Macs use the
browser client. Mac apps are Developer ID signed
and notarized; Linux apps are unsigned. Releases include DMG/ZIP (macOS),
AppImage/DEB (Linux), installer archives, SHA-256 checksums, and a Sigstore
signature over the checksums. Assets publish to `RELEASES_REPO` (default
`ZechCodes/build-releases`), followed by the `desktop-latest/version.txt` pointer.
Desktop releases never replace the bridge's latest release.

Before signing checksums, publication requires all nine expected distributables
from `scripts/desktop-release-assets.mjs`: three macOS ARM64 assets and three
assets for each Linux architecture. Missing, empty or unexpected files fail
publication. Version 0.1.2 stages downloads, validation, checksums, signing and
uploads in a fresh `$RUNNER_TEMP/desktop-release.XXXXXX` directory so checkout's
`assets/brand` cannot enter the release manifest. The failed `desktop-v0.1.0`
and `desktop-v0.1.1` source tags remain unchanged.

## Apple and release secrets

Configure the same repository secrets used for bridge releases:
`APPLE_CERTIFICATE_P12` (base64-encoded Developer ID Application certificate),
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_APP_PASSWORD` (app-specific
password), `APPLE_TEAM_ID`, and `RELEASES_TOKEN` (write access to the releases
repository). Missing signing credentials fail the desktop release rather than
publishing unsigned assets. Local builds need none of these secrets.
Set the website's `RELEASES_REPO` environment variable to the same repository
if you override the workflow variable. Website-served installers use that
repository by default; `BUILD_RELEASES_REPO` overrides it for a single install.
