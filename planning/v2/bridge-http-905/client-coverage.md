# Build client coverage acceptance

This is the release inventory for #902's usable REST/SSE client, not an assertion
that the routes exist. The lifecycle OpenAPI skeleton and first task resource
slice alone are an automation API. All rows below must have explicit typed
resource routes, capabilities, adapter tests and UI acceptance before the Build
client advertises the corresponding HTTP views. No POST /rpc fallback is allowed.

Existing verb families below are inventory references, not proposed new sync
names. Final route manifest must enumerate exact operations from the typed
registry AND legacy session-scoped UI calls; a family wildcard is not runtime
permission. An unexported method receives explicit unsupported semantics. Scope
and authenticated principal checks run before dispatch for every route.

| User interaction | Required HTTP resources / methods | Existing execution boundary to inventory | First resource slice sufficient? |
| --- | --- | --- | --- |
| Open device, reconnect, capabilities, diagnostics | sessions POST/PATCH/DELETE, greeting GET, health/diagnostics reads | session.hello, ping, bridge.stats, epoch/capability contract | Lifecycle only; diagnostics still required. |
| Inbox, project/workspace/agent directories, watched/unread badges | directory GETs, watched/read-mark resources, summaries GETs | project.*, workspace.*, agent.*, board.list and read/watch mutations; preserve #887 legacy refresh until cutover | No; directory lists alone omit Inbox/read state. |
| Tasks/board/filtering, create/edit/move/close/reopen/watch | project tasks GET/POST, task GET/PATCH, task column/state/watch resources PUT/DELETE; choices/summary GET | tasks.*, exact existing scalar/plural filters and negotiated bounded summary mode | No; create/update/read alone omit moves, watch/read and filter menus. |
| Comments, attachments, references, dependencies | comments POST/GET, attachment chunks PUT/GET, links/blockers resources PUT/DELETE, exact lookup GET | tasks.comment, task links/attachments/waiting-on and reference queries; #900/#901 only when landed | No; preserve large-body/history usability and reference membership. |
| Conversation reading and user messages/options | agent conversation pages/activity/operations GET, messages POST, read cursor PUT | thread.*, entity reply/read/options operations | No; required for the product's core interaction. |
| Agent management and runtime controls | workspace/project agents POST, agent settings PATCH, start/interrupt/stop resources POST, deletion DELETE | agent.*, entity.*, conversation settings and reset operations | No; unknown outcome must not replay messages or starts. |
| Add project/workspace/source, rename/lock/retry/reclaim/delete | project/workspace/source resources GET/POST/PATCH/DELETE, lock PUT/DELETE, source preparations/retries POST | project.*, workspace.*, worktree lifecycle and source management | No; preserve original-source protection and source-path preconditions. |
| Files/editor/media/uploads | scoped file/tree/content GET, file content PUT with existing preconditions, directories/files POST, upload chunk PUT | fs.*, binary/media range and chunk contracts | No; no arbitrary absolute filesystem paths or new URL fetching. |
| Changes/commits/reviews, line/file comments and Git actions | changes/commits/patch resources GET, review snapshots/comments/actions POST/GET, Git action resources POST | git.*, tasks.review.*, capture.*; source-qualified selectors | No; do not expose an arbitrary shell-command endpoint. |
| Settings/models/harness credentials/compaction | settings PATCH/GET, catalogs GET, refresh jobs POST, policies PATCH | settings.*, models.*, harnesses.*, compaction settings; existing capability gates | No; avoid dropping credential detection/configuration UI. |
| Bridge updates and maintenance | update-status GET, update-check POST, installation command POST | bridge.update_status/check_update/install_update, existing confirmation policy | No; transport does not grant installation authorization. |
| Live changes/receipts and cache refresh | subscriptions POST/DELETE, events GET, operations GET | changes.subscribe, changes.unsubscribe, writes.status/subscriptions, all published event kinds used by selected views | Initial live stream needs event coverage; no durable cursor. |
| Device account settings/pairing/revoke/notifications | hosted existing account resource calls; never direct device bearer for hosted API | spa/src/api.js and hosted routes, existing push key registration remains its own contract | Existing hosted path remains required. |
| Terminal shell/input/resize/ACK/reset | Separate authenticated WebRTC terminal path in this delivery; no SSE terminal substitute | term.*, term channel, independent session key proof and parent binding per #904/#909 | Explicit exception; no HTTP terminal capability until separate reviewed protocol. |

The implementation inventory must be mechanically checked against the actual
SPA consumer method set, not only Rust's typed table: current session hello,
terminal and several agent calls are legacy. Include lazy-loaded views and
background reads. Exported OpenAPI operationId maps to exactly one normalized
Invocation and lane; aliases cannot bypass authorization or journal semantics.

Read-heavy resources with complex selectors may use a named query resource
(POST /v1/projects/{project}/task-queries) rather than an unbounded GET URL.
Creating that query does not execute an arbitrary supplied method; its typed
selector grammar and response/page limits are fixed by the resource contract.
Use domain actions such as POST /v1/agents/{agent}/interruptions where an
imperative operation has no natural CRUD representation. These are explicitly
named resource operations, not a method string + arbitrary params tunnel.

Acceptance for the **Build connection** (not merely API availability):

1. From Add by URL, a current account can discover/verify/connect to its enabled
   paired bridge and use Inbox, Tasks, conversations, workspaces, Files, Changes,
   reviews, agent controls and Settings through implemented advertised routes.
   Refresh/reconnect retains cache and existing current-state recovery behavior.
2. Every advertised interactive action has a route and meaningful integration
   test; unsupported older bridge capabilities are visibly gated before use.
   Never select HTTP and discover an unsupported operation only after a click.
3. A terminal explicitly reports that it uses WebRTC and whether that path is
   available; inability to negotiate WebRTC affects the terminal, not the HTTP
   views. No automatic transport switching resends terminal input or uncertain
   domain writes. Terminal route exception must be visible in capability data.
4. Domain errors/preconditions/receipts/operation status and epoch fences behave
   the same over WebRTC and HTTP. Mutation retry after a lost HTTP response uses
   the same principal and #739 operation ID over either supported transport.
5. Small/single-project/large profiles keep bounded admission, stream memory,
   cache publication and unrelated-client latency. #887 behavior is consumed
   behind its capabilities only when landed, with no dependency on its ETA.

Staging does not reduce final task acceptance. If a route family is intentionally
deferred beyond final delivery, coordinator must explicitly revise the advertised
client scope and product flow before enabling that connection option.
