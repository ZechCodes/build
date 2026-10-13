# Build Desktop

Build Desktop is a sandboxed Electron shell for the hosted Build web client.
Published builds support Apple Silicon (ARM64) Macs and Linux x86_64/ARM64.
Intel Macs can use the browser client.
It connects to the user's separately installed bridge through Build's existing
end-to-end encrypted relay. The desktop app does not install, launch, or access
the bridge or the user's files.

Sign-in opens the existing passkey flow in the system browser through Skrift's
OAuth 2.0 Authorization Code flow with PKCE. Access and refresh tokens remain in
the Electron main process and are never exposed to the hosted renderer.

The refresh token is encrypted with Electron `safeStorage` and saved as
`refresh-token.enc` under the app's `userData` directory with mode 0600 (on
POSIX systems). Startup silently refreshes it before opening the app. Each
rotated token atomically replaces the previous file. If secure storage is
unavailable, including Linux's `basic_text` fallback, credentials remain in
memory and the next launch requires browser sign-in. See Electron's
[safeStorage documentation](https://www.electronjs.org/docs/latest/api/safe-storage)
for the OS key providers and protection boundaries.

The server's pinned Skrift 0.2.0a10 implementation gives refresh tokens a
**30-day sliding lifetime**, and access tokens a 15-minute lifetime. Each
successful refresh issues a replacement valid for another 30 days. With secure
storage available, you stay signed in across launches for up to 30 days since
the last successful refresh; using the app regularly extends that period.
Expiry or revocation (`invalid_grant`) clears the saved token and opens browser
sign-in. A network/server failure retains the token and offers a refresh retry;
it does not start browser sign-in. A failed response after server rotation can
still require sign-in on retry because the previous token is already invalid.

**Account → Sign Out** clears the saved token, credentials and refresh timer,
closes the app window, and quits. This signs out this desktop installation;
it does not sign out the system browser or revoke other sessions. If the disk
cannot be cleared, the app reports that sign-out failed before quitting.

Authentication regression tests stub Electron, encryption, token HTTP responses,
browser handoff and timers. They never start Electron or perform a real sign-in.

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
The macOS DMG and ZIP targets default to ARM64.
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
