# Releasing the desktop app

The bridge's release procedure, and how users verify both products' downloads,
are in [`deploy/README.md`](../deploy/README.md#releasing-the-bridge). What
changed in each release is recorded in [`CHANGELOG.md`](../CHANGELOG.md).

## Electron releases

The `Release the desktop app` GitHub workflow runs on `desktop-vX.Y.Z` tags.
The tag must match `desktop/package.json` and its lockfile. It builds native
macOS ARM64/Intel and Linux ARM64/x86_64 apps. Mac apps are Developer ID signed
and notarized; Linux apps are unsigned. Releases include DMG/ZIP (macOS),
AppImage/DEB (Linux), installer archives, SHA-256 checksums, and a Sigstore
signature over the checksums. Assets publish to `RELEASES_REPO` (default
`ZechCodes/build-releases`), followed by the `desktop-latest/version.txt` pointer.
Desktop releases never replace the bridge's latest release.

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
