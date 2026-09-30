# Build Desktop

Build Desktop is a sandboxed Electron shell for the hosted Build web client.
It connects to the user's separately installed bridge through Build's existing
end-to-end encrypted relay. The desktop app does not install, launch, or access
the bridge or the user's files.

Sign-in opens the existing passkey flow in the system browser through Skrift's
OAuth 2.0 Authorization Code flow with PKCE. Access and refresh tokens remain in
the Electron main process and are never exposed to the hosted renderer.

## Develop

```bash
npm install
npm test
npm start
```

`npm start` opens `https://getbuild.ing/app/`. Development builds expose
Chromium developer tools; packaged builds do not.

## Package

Build an unsigned local installer for the current platform, including locked
dependency installation (run from the repository root):

```bash
node scripts/build-desktop.mjs
```

Output is in `desktop/dist/`. Add `--dir` for an unpacked application.
This local script ignores signing credentials and disables notarization;
on macOS it uses an ad-hoc signature and disables hardened runtime.
See the root [contributing guide](../CONTRIBUTING.md#build-locally) for
prerequisites, and [`docs/releasing.md`](../docs/releasing.md) for the signed
Mac release workflow.

For credential-aware packaging directly from this directory, use `npm run dist`,
or `dist:mac`, `dist:win`, or `dist:linux` on the corresponding signing host.
When no macOS signing identity is available, this lower-level command falls back
to an ad-hoc signature; use the root script for runnable local builds with
hardened runtime disabled. A configured `CSC_LINK`,
`CSC_NAME`, or keychain identity takes precedence for production signing.
Production macOS releases must also be notarized before distribution.
