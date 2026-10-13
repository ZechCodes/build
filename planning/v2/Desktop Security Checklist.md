# Desktop security checklist

**Status:** 100/100 (10/10 scoped controls verified)
**Verified:** 2026-10-13, #615, including the snapshot-1 and snapshot-2 Astra review fixes.

Scope: the desktop shell's authentication, session persistence, renderer and
navigation boundaries. Evidence uses Node tests with stubbed Electron, OS
storage and HTTP. This is not evidence of a live Keychain/DPAPI/keyring sign-in,
installer signing, or notarization. No installed app or real sign-in was run.

| # | Control | Score | Evidence |
| --- | --- | --- | --- |
| 1 | Browser sign-in uses the public desktop client, PKCE S256 and a matching callback state. | 10/10 | `desktop/test/oauth.test.mjs`: RFC challenge, registered authorization URL, callback URL/state rejection. `main-auth.test.mjs`: first sign-in follows PKCE callback and persists credentials. |
| 2 | Only the refresh token is persisted, using `safeStorage` after app ready; credentials never enter renderer options or browser URLs. | 10/10 | `src/main.mjs`: `app.whenReady`, `useTokens` and main-process variables; `src/token-store.mjs`: `encryptString`/`decryptString`; `main-auth.test.mjs`: silent launch and restored access header, browser receives authorization URL only. |
| 3 | Saved ciphertext stays under `app.getPath("userData")`, and replacements use private files, never append. | 10/10 | `token-store.test.mjs`: new-store reload, long-to-short rotation, 0600 after replacing a 0644 file, no leftover temporary file, symlink replacement leaves target untouched. `token-store.mjs`: exclusive random temporary file, fsync, atomic rename. POSIX mode checks are platform-specific. |
| 4 | Unavailable encryption or Linux `basic_text` never writes credentials. Successful memory-only sign-in removes any previously saved account; deletion failure is reported. Corrupt ciphertext cannot restore credentials. | 10/10 | `token-store.test.mjs`: unavailable/basic_text forbid encrypt/decrypt and leave no new file; corrupt file discarded. `main-auth.test.mjs`: successful account B sign-in with unavailable encryption, basic_text or encryption failure removes account A’s file; a later secure launch opens browser sign-in and makes no refresh request. Each fallback reports deletion failure and opens no account window. Encryption failure logs no exception/secret. [Electron documents basic_text as unprotected](https://www.electronjs.org/docs/latest/api/safe-storage). |
| 5 | Startup and scheduled refresh persist each rotation before using it; only one refresh request can run at once. | 10/10 | `main-auth.test.mjs`: silent launch, second launch uses last rotated token, scheduled refresh sends latest token; direct overlapping calls to the actual private refresh function share one pending exchange, and the next request uses its replacement. Negative control: a transient loader removing only the refreshInFlight guard makes this regression exit 1 (three requests instead of two); normal code passes. Activation while startup is pending also sends no browser handoff. `main.mjs`: `refreshInFlight`, synchronous save before access-token assignment and timer. |
| 6 | Refused refresh (`invalid_grant`) clears disk/in-memory credentials and old account window before browser reauthentication; transient failures retain the token. | 10/10 | `main-auth.test.mjs`: startup/scheduled invalid_grant, transport/server/incomplete-response failures, Try Again retries refresh without browser. Main reports storage deletion failures instead of claiming success. |
| 7 | Native Sign Out removes saved credentials, clears headers/timer, aborts token traffic and retires the renderer before bounded default-session storage/cache cleanup. Native Quit waits for cleanup and failure reporting. Token traffic cannot mutate the persistent window session. | 10/10 | `main-auth.test.mjs`: deferred storage/cache cleanup; native `before-quit` veto during either wait and the failure dialog, final quit allowed, cleanup-then-quit and ordinary quit allowed; storage/cache errors and five-second timeouts attempt both operations and report failure. Pending/rejected failure dialogs release quit; the dialog has a native renderer-free parent and AbortSignal (unparented macOS dialogs block synchronously). Refresh and code exchanges are aborted; modeled late Set-Cookie/cache responses deliberately ignore abort and cannot change the default session or resurrect credentials. All token requests use a nonempty partition without `persist:`, cache disabled, credentials omitted and no-store caching. Existing old-authentication-dialog, credential/timer and late-generation regressions remain green. `main.mjs`: tracked AbortControllers through JSON consumption, isolated `session.fetch`, per-operation five-second deadlines (at most 15 seconds of async waits), and completion flag permits final quit. A timed-out deletion may leave data behind and is reported as failure. Sign-out is local, not global server revocation. |
| 8 | Hosted renderer remains sandboxed, isolated, without Node, webviews or production devtools; sensitive packaged Electron fuses remain disabled. | 10/10 | `desktop/test/security-policy.test.mjs`: window preferences, webview rejection, packaged fuse configuration. |
| 9 | Only exact desktop app navigation remains inside Electron; external HTTP/HTTPS/mailto links reach the browser without credentials and unsafe schemes are denied. | 10/10 | `security-policy.test.mjs`: exact paths/prefix lookalikes, will-navigate/will-redirect/window-open handoff, denied schemes. |
| 10 | Authorization is stripped on every request and added only to desktop main-frame and same-origin API XHR; permissions remain restricted to same-origin notifications. | 10/10 | `security-policy.test.mjs`: all-URL hook, header casing/redirect stripping, API versus document/resource allowlist, notification-only permission policy. |

**Total: 100/100.**

Server behavior was checked in the pinned vendored wheel
`skriftapp/vendor/skrift-0.2.0a10-py3-none-any.whl`:
`skrift/controllers/oauth2.py` sets access TTL 900 seconds and refresh TTL
2,592,000 seconds (30 days). `_handle_refresh_token` revokes the old JTI and
issues a fresh 30-day refresh token in the same family. Reuse of a revoked token
revokes its family; invalid/expired tokens return `invalid_grant`.
`skrift/auth/tokens.py` checks expiry. Thus the desktop stays signed in up to
30 days after its last successful refresh, extended by regular use. Refresh
requests are serialized; transport failures are not automatically retried. A
user-requested retry after an ambiguous server rotation may require sign-in.

Round-3 source references: Electron 44.1.1 documents [in-memory partitions and
session.fetch](https://github.com/electron/electron/blob/v44.1.1/docs/api/session.md),
[before-quit prevention](https://github.com/electron/electron/blob/v44.1.1/docs/api/app.md#event-before-quit),
and the [macOS parent requirement for abortable message boxes](https://github.com/electron/electron/blob/v44.1.1/docs/api/dialog.md).
The late-cookie/cache evidence is a source-backed model in the Electron stub,
not a live Electron reproduction or a claim that the endpoint emits cookies.
