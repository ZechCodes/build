# Desktop security checklist

**Status:** 100/100 (10/10 scoped controls verified)
**Verified:** 2026-10-16, #635 window chrome, retaining #615's snapshot-1 through snapshot-4 authentication review fixes.

Scope: the desktop shell's authentication, session persistence, renderer and
navigation boundaries and native window-control geometry. Evidence uses Node
tests with stubbed Electron, OS storage and HTTP, plus Chromium tests of the
standard Window Controls Overlay signal. This is not evidence of a live Keychain/DPAPI/keyring sign-in,
installer signing, or notarization. No installed app or real sign-in was run.

| # | Control | Score | Evidence |
| --- | --- | --- | --- |
| 1 | Browser sign-in uses the public desktop client, PKCE S256 and a matching callback state. | 10/10 | `desktop/test/oauth.test.mjs`: RFC challenge, registered authorization URL, callback URL/state rejection. `main-auth.test.mjs`: first sign-in follows PKCE callback and persists credentials. |
| 2 | Only the refresh token is persisted, using `safeStorage` after app ready; credentials never enter renderer options or browser URLs. | 10/10 | `src/main.mjs`: `app.whenReady`, `useTokens` and main-process variables; `src/token-store.mjs`: `encryptString`/`decryptString`; `main-auth.test.mjs`: silent launch and restored access header, browser receives authorization URL only. |
| 3 | Saved ciphertext stays under `app.getPath("userData")`, and replacements use private files, never append. | 10/10 | `token-store.test.mjs`: new-store reload, long-to-short rotation, 0600 after replacing a 0644 file, no leftover temporary file, symlink replacement leaves target untouched. `token-store.mjs`: exclusive random temporary file, fsync, atomic rename. POSIX mode checks are platform-specific. |
| 4 | Unavailable encryption or Linux `basic_text` never writes credentials. Successful memory-only sign-in removes any previously saved account; deletion failure is reported. Corrupt ciphertext cannot restore credentials. | 10/10 | `token-store.test.mjs`: unavailable/basic_text forbid encrypt/decrypt and leave no new file; corrupt file discarded. `main-auth.test.mjs`: successful account B sign-in with unavailable encryption, basic_text or encryption failure removes account A’s file; a later secure launch opens browser sign-in and makes no refresh request. Each fallback reports deletion failure and opens no account window. Encryption failure logs no exception/secret. [Electron documents basic_text as unprotected](https://www.electronjs.org/docs/latest/api/safe-storage). |
| 5 | Startup and scheduled refresh persist each rotation before using it; only one refresh request can run at once. | 10/10 | `main-auth.test.mjs`: silent launch, second launch uses last rotated token, scheduled refresh sends latest token; direct overlapping calls to the actual private refresh function share one pending exchange, and the next request uses its replacement. Negative control: a transient loader removing only the refreshInFlight guard makes this regression exit 1 (three requests instead of two); normal code passes. Activation while startup is pending also sends no browser handoff. `main.mjs`: `refreshInFlight`, synchronous save before access-token assignment and timer. |
| 6 | Refused refresh (`invalid_grant`) clears disk/in-memory credentials and old account window before browser reauthentication; transient failures retain the token. | 10/10 | `main-auth.test.mjs`: startup/scheduled invalid_grant, transport/server/incomplete-response failures, Try Again retries refresh without browser. Main reports storage deletion failures instead of claiming success. |
| 7 | Native Sign Out removes saved credentials, clears headers/timer, aborts token traffic and retires the renderer before bounded default-session storage/cache cleanup. Native Quit waits for cleanup and failure reporting. Authentication UI, credential restoration and window/browser helpers reject work during Sign Out. Token traffic cannot mutate the persistent window session. | 10/10 | `main-auth.test.mjs`: deferred storage/cache cleanup; native Quit veto during either wait and failure reporting, final and ordinary quit allowed; errors/timeouts attempt both operations and report failure. Dialog has a renderer-free native parent and AbortSignal; pending/rejected dialogs and parent dismissal release quit safely. Late refresh/code cookies and cache cannot change the default session, and requests are aborted through JSON consumption. Direct private-helper probes independently exercise the post-dialog retry callback, window reopening, startup credential loading after deletion failure, and browser handoff. Along with the deferred browser-rejection test for the pre-dialog check, removing each of these five guards separately via a transient loader causes exactly its expected test to fail (49 pass/1 fail, exit 1); normal authentication suite: 50 pass, exit 0. Earlier activation/protocol/old-dialog behavior tests alone did **not** distinguish those guards because other checks masked removal. Redundant signingOut checks were removed from callback delivery (pending authorization cleared), second-instance window actions (main window destroyed/null), activation (credentials cleared and startup helper guarded), refresh entry (refresh token cleared), and window-all-closed (central before-quit veto). A separate test observes restore/show/focus before Sign Out and no window actions afterwards. Timer cancellation remains resource cleanup; ticking timers proves no extra token requests, not independent sensitivity to removing clearTimeout. Prior generation, isolation, native-quit and cleanup regressions remain green. Each async cleanup/report wait has a five-second deadline (at most 15 seconds); timeouts report failure and may leave data behind. Sign-out is local, not global revocation. |
| 8 | Hosted renderer remains sandboxed, isolated, without Node, webviews or production devtools; sensitive packaged Electron fuses remain disabled. Window chrome exposes only standard read-only WCO geometry. | 10/10 | `desktop/test/security-policy.test.mjs`: window preferences, webview rejection, packaged fuse configuration, macOS hidden title bar/44px overlay/native controls and identical renderer security options. `spa/test/windowControlsOverlay.test.js` and `spa/test/browser/windowControlsOverlayLayout.test.js`: inactive browser/PWA signal leaves geometry and drag behavior unchanged; native geometry reserves only the top-left header, follows pin/resize/fullscreen, and excludes interactive targets and menus from drag. No preload, IPC, custom desktop identity marker or privileged renderer API was added. |
| 9 | Only exact desktop app navigation remains inside Electron; external HTTP/HTTPS/mailto links reach the browser without credentials and unsafe schemes are denied. | 10/10 | `security-policy.test.mjs`: exact paths/prefix lookalikes, will-navigate/will-redirect/window-open handoff, denied schemes. |
| 10 | Authorization is stripped on every request and added only to desktop main-frame and same-origin API XHR; permissions remain restricted to same-origin notifications. | 10/10 | `security-policy.test.mjs`: all-URL hook, header casing/redirect stripping, API versus document/resource allowlist, notification-only permission policy. |

**Total: 100/100.**

Window chrome evidence is simulated: Chromium paints production SPA headers
against a stubbed `navigator.windowControlsOverlay` and optional colored-circle
probes, not native traffic lights. Electron's pinned [macOS implementation](https://github.com/electron/electron/blob/v44.1.1/shell/browser/native_window_mac.mm#L1735)
returns the content rectangle to the right of the native button container and
an empty rectangle in native fullscreen. No fixed button position is supplied:
Electron [centers the native button frame using its actual height](https://github.com/electron/electron/blob/v44.1.1/shell/browser/ui/cocoa/window_buttons_proxy.mm#L181)
within the 44px overlay. The probe circles and test rectangle's x=84 are synthetic,
not measured AppKit geometry. CSS prefers `env(titlebar-area-x)`;
the same API's x coordinate supplies a fallback. Only Zech's signed macOS app
can confirm native positioning, close/minimize/fullscreen and restore, dragging
and double-click behavior, and alignment while resizing or changing display scale.

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
