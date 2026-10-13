# Desktop security checklist

**Status:** 70/100; controls 4, 5 and 7 are pending the #615 review fixes.
**Verified:** 2026-10-13, #615.

Scope: the desktop shell's authentication, session persistence, renderer and
navigation boundaries. Evidence uses Node tests with stubbed Electron, OS
storage and HTTP. This is not evidence of a live Keychain/DPAPI/keyring sign-in,
installer signing, or notarization. No installed app or real sign-in was run.

| # | Control | Score | Evidence |
| --- | --- | --- | --- |
| 1 | Browser sign-in uses the public desktop client, PKCE S256 and a matching callback state. | 10/10 | `desktop/test/oauth.test.mjs`: RFC challenge, registered authorization URL, callback URL/state rejection. `main-auth.test.mjs`: first sign-in follows PKCE callback and persists credentials. |
| 2 | Only the refresh token is persisted, using `safeStorage` after app ready; credentials never enter renderer options or browser URLs. | 10/10 | `src/main.mjs`: `app.whenReady`, `useTokens` and main-process variables; `src/token-store.mjs`: `encryptString`/`decryptString`; `main-auth.test.mjs`: silent launch and restored access header, browser receives authorization URL only. |
| 3 | Saved ciphertext stays under `app.getPath("userData")`, and replacements use private files, never append. | 10/10 | `token-store.test.mjs`: new-store reload, long-to-short rotation, 0600 after replacing a 0644 file, no leftover temporary file, symlink replacement leaves target untouched. `token-store.mjs`: exclusive random temporary file, fsync, atomic rename. POSIX mode checks are platform-specific. |
| 4 | Unavailable encryption or Linux `basic_text` never writes credentials; corrupt ciphertext cannot restore credentials. | 0/10 pending | `token-store.test.mjs`: unavailable/basic_text forbid encrypt/decrypt and leave no new file; corrupt file discarded. `main-auth.test.mjs`: unavailable encryption authenticates only in memory; encryption failure removes stale token, uses memory, logs no exception/secret. [Electron documents basic_text as unprotected](https://www.electronjs.org/docs/latest/api/safe-storage). |
| 5 | Startup and scheduled refresh persist each rotation before using it; only one refresh request can run at once. | 0/10 pending | `main-auth.test.mjs`: silent launch, second launch uses last rotated token, scheduled refresh sends latest token, repeated activation while startup refresh is pending sends one request and no browser handoff. `main.mjs`: `refreshInFlight`, synchronous save before access-token assignment and timer. |
| 6 | Refused refresh (`invalid_grant`) clears disk/in-memory credentials and old account window before browser reauthentication; transient failures retain the token. | 10/10 | `main-auth.test.mjs`: startup/scheduled invalid_grant, transport/server/incomplete-response failures, Try Again retries refresh without browser. Main reports storage deletion failures instead of claiming success. |
| 7 | Native Sign Out removes saved credentials, clears access headers/timer, destroys the account window, and ignores late refresh/code-exchange responses. | 0/10 pending | `main-auth.test.mjs`: sign-out clears disk/headers/window and cancels scheduled refresh; late refresh and authorization-code exchange cannot resurrect credentials. `main.mjs`: generation check and deletion-error dialog before quit. Sign-out is local, not global server revocation. |
| 8 | Hosted renderer remains sandboxed, isolated, without Node, webviews or production devtools; sensitive packaged Electron fuses remain disabled. | 10/10 | `desktop/test/security-policy.test.mjs`: window preferences, webview rejection, packaged fuse configuration. |
| 9 | Only exact desktop app navigation remains inside Electron; external HTTP/HTTPS/mailto links reach the browser without credentials and unsafe schemes are denied. | 10/10 | `security-policy.test.mjs`: exact paths/prefix lookalikes, will-navigate/will-redirect/window-open handoff, denied schemes. |
| 10 | Authorization is stripped on every request and added only to desktop main-frame and same-origin API XHR; permissions remain restricted to same-origin notifications. | 10/10 | `security-policy.test.mjs`: all-URL hook, header casing/redirect stripping, API versus document/resource allowlist, notification-only permission policy. |

**Total: 70/100 pending review fixes.**

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
