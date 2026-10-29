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
memory; a successful new sign-in removes any previously saved account’s token,
and the next launch requires browser sign-in. A deletion failure is reported
before opening the account window. See Electron's
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

**Account → Sign Out**, or **Sign out** on the welcome screen or in Settings,
clears the saved token, credentials and refresh timer,
retires the app window, then awaits clearing the default session’s cookies,
localStorage, IndexedDB and HTTP cache before quitting. Native Quit (including
Cmd-Q or the dock) waits for this cleanup and any failure report. Each storage,
cache and failure-dialog wait has a five-second deadline, so Sign Out waits at
most 15 seconds for those operations. A timeout is reported as a cleanup
failure, not success. The failure dialog uses a native parent without a
renderer so it remains asynchronous and abortable on macOS.

Token exchanges use a separate in-memory session with cookies omitted and HTTP
caching disabled. Sign Out aborts outstanding exchanges before clearing the
persistent window session; even a response that races cancellation cannot
rewrite that session. This signs out this desktop installation; it does not
sign out the system browser or revoke other sessions. If saved credentials,
session storage or the HTTP cache cannot be cleared, the app reports that
sign-out failed before quitting.

The hosted controls target `/auth/logout`. Electron intercepts that exact
same-origin path and runs the same native cleanup as the Account menu, rather
than handing the link to the system browser. In the web app, it opens the
server's logout confirmation as usual.

Authentication regression tests stub Electron, encryption, token HTTP responses,
browser handoff and timers. They never start Electron or perform a real sign-in.

On macOS, the native title bar is hidden and the native traffic lights sit
inline with the app's 44px top bars. Electron centers them using the actual
native button height and retains the OS horizontal inset. Its Window Controls
Overlay API reports their reserved space to the web client, without a preload or IPC
bridge. macOS native fullscreen temporarily disables the overlay, so the app
removes that reserved space until the window leaves fullscreen. Linux and
Windows retain their native title bar and window controls.

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

## Optional direct bridge HTTPS

The hosted client can use `window.buildBridgeHttp` to discover an account-owned,
paired bridge and open its optional HTTPS interface. Main performs discovery,
asks the fixed hosted account service to verify endpoint ownership, and keeps
separate device credentials in memory. Entered addresses never receive desktop
OAuth credentials. Only the current hosted main frame can invoke the typed
preload API; it cannot choose HTTP methods, paths or headers. Resource requests
resolve through the reviewed version-1 manifest packaged with the app. A partial
or incompatible bridge manifest fails connection compatibility explicitly.

The direct connection uses normal HTTPS certificate verification and rejects
redirects. It requires a trusted certificate and a reachable configured bridge;
there is no certificate bypass. The terminal continues to require its separate
WebRTC connection. Native HTTP operations do not alter hosted OAuth header
injection, renderer web security, browser permissions or hosted CSP.

Application SSE is transient. The native adapter validates the opening and fresh
subscription identity before forwarding events, bounds unacknowledged delivery
at 128 events and 1 MiB, and closes stalled delivery after ten seconds. The
renderer acknowledges events after applying them; this sequence is local flow
control, never a durable resume cursor. Reconnection recreates subscriptions and
refreshes authoritative current state. Lost requests are not automatically
replayed, and failures after dispatch report an uncertain outcome.

Every OAuth access-token rotation retires the old native bridge generation and
requires new account verification. The account service binds bridge credential
families to that verified access-token issuance, not to a stable installation
identity. Rotation and Sign Out fence handles and abort requests/streams first,
then attempt bounded revocation of only the tracked families with the previous
OAuth credential. Sign Out clears this app's credentials immediately. If the
server cannot be reached, previously issued access may remain valid until it
expires: a device token lasts at most five minutes, capped by its original parent
OAuth expiry; the old OAuth issuance itself may remain authorized until its
original expiry. Successful remote revocation takes effect at the next bridge
verification within thirty seconds. Local cleanup proceeds if revocation fails.

Transport/IPC tests use isolated fixtures; HTTPS tests use real loopback TLS
sockets and a throwaway trusted certificate. Main authentication tests inject a
minimal test-only resource manifest and do not establish server route parity.
