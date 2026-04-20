# 00 — Diagnosis of v1

## Three architectural smells

All "not operational" symptoms trace back to these three.

### 1. One flat mutable `state` object

`frontend/src/dashboard/state.js` is ~45 properties in a single object. Every
module reads/writes it directly. Two problems:

- **Cross-channel and singleton state share the namespace.** `chatMessages`
  is `Map<channelId, msgs>` (per-channel) but `filesCurrentPath`,
  `terminalCurrentBlock`, `filesChannelId` are singletons. When you switch
  channels, you must manually swap singleton-scoped view state. That's why
  `selectChannel()` is 187 lines of wiring and why `_filesSelfHealed` exists
  as a race-band-aid.
- **No subscription.** Every module calls `renderX()` manually when it thinks
  something changed. If two things change, two render calls happen; if one
  thing changes and a dependent forgets to re-render, the UI goes stale.

### 2. `legacy.js` is a re-export hub, not a bootstrap

`frontend/src/dashboard/legacy.js` imports ~100 functions from every module,
re-exports them, and leaks six functions onto `window` (`switchTab`,
`renderMessages`, `highlightLine`, `updateStopButton`, `updatePlanModeUI`,
`fetchDevices`). The module boundaries are fake — anything can call anything
via `window` or via the shared state object.

### 3. E2EE handlers mix transport, domain, and view

`e2ee/handlers/agent.js` (243 lines) receives a decrypted frame and:

1. Mutates `state.chatMessages` (domain state).
2. Calls `appendMessage()` and `handleNewMessageScroll()` (view).
3. Calls `renderChannelList()` and `updateTasksBadge()` (other domain views).
4. Sets `state.channelAgentActive` (presence).
5. Calls `updateStopButton()`, `updatePlanModeUI()` (UI state).

One event dispatches to 5+ concerns inline. No bus; no way to add a
subscriber without editing the handler.

## Symptom → root cause map

| Symptom                              | Root cause                                                       |
| ------------------------------------ | ---------------------------------------------------------------- |
| Chat scroll drifts between channels  | `console/view.js` scroll anchors are globals that leak on switch |
| `_filesSelfHealed` race band-aid     | `filesChannelId` is a global, not per-channel view state         |
| Terminal completions get stale       | `terminalCompletionPending` is a global                          |
| Tasks can't CRUD                     | No domain store for todos; only a render function                |
| E2EE reconnect double-teardown risk  | Retry timer and SSE handler both initiate reconnect              |
| Review flow stubs (toast only)       | No owner for the feature (no `ReviewView`, no `reviewStore`)     |
| Reload loses message history         | Store is in-memory only                                          |
| Complications UI read-only           | No action dispatch from view back to transport                   |
| `renderChannelPanel()` polls every 15s | No reactive subscription; polling hides stale state            |

## Code smell inventory (as of scaffold)

- `state.js` has `_filesSelfHealed`, `_pendingChannelId`, `_navigatingHistory`,
  `_openDropdown`, `_reconnectPillTimer`, `_unreadHighlightUntil`,
  `_unreadHighlightLastSeen` — underscore-prefixed internal flags are a
  signal that the module boundary is wrong.
- `channels/list.js` exports `renderChannelList → renderChannelPanel` as a
  no-op shim, and `updateAggregateBadge` that's been removed — back-compat
  shims for features that moved.
- `app/init.js` contains a `relocateChatLayout()` IIFE that physically moves
  `#tab-chat` DOM into `#chat-overlay-body` on boot. Layout-as-JS-side-effect.
- `e2ee/connect.js` has a comment: `External deps via window during the
  transition: window.fetchDevices — defined in legacy.js (will move to
  app/init.js in Wave D).` The transition stalled.
- `e2ee/handlers/files.js` self-heal comment: `Accept if it matches
  state.filesChannelId, OR if state.filesChannelId is unset but this is the
  currently-active chat channel (self-heal race).`

## What v2 must preserve

- The E2EE transport (`vendor/e2ee.js`) and its event vocabulary — it works.
- The SSE (`skrift.notifications`) reconnect signal — it works.
- The backend `/api/devices/` and E2EE relay — unchanged.
- The markdown renderer (`vendor/markdown.js`) and syntax highlighter
  (`files/syntax.js`) — port as-is into v2.

v2 is a **frontend-only** rewrite. No backend changes required.
