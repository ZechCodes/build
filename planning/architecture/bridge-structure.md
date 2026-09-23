# Bridge service boundaries (#86)

The bridge remains one device daemon. It owns work that must continue with no browser connected: agent turns, routing, retries, compaction, active tracker and conversation work, workspace lifecycle, recovery, and notifications. The SPA chooses user-facing actions and renders cached facts; it does not schedule or interpret these workflows. This plan builds on the completed [modularization strategy](../v2/Bridge%20Modularization%20Strategy.md) and its [concurrency rule](../v2/Bridge%20Concurrency%20Spec.md), rather than restarting the file split. Under the binding [strict P2P topology](../v2/Strict%20P2P%20Transport%20Spec.md), application RPC and events travel inside E2EE WebRTC DataChannels over a direct or TURN path. The relay only provides authentication, rendezvous, and signaling; skriftapp provides auth, static assets, presence, and content-free push.

## Target shape

```text
bridge/src/
  app.rs                         composition root: build dependencies, start pumps
  app/rpc.rs, api/v1/, app/mcp.rs  wire adapters: parse, authenticate, reply
  services/
    contracts.rs                 internal typed commands, queries, facts, errors
    agents/                     session orchestration, delivery, usage, compaction
    tracker/                    active tracking, inbox, activity, attention
    routing/                    captures, router decisions, project-agent work
    workspaces/                 project/workspace/checkout lifecycle coordination
    notifications/              attention decisions, durable send intent, push
  store/, thread/, operation/    canonical records and conversation/receipt writes
  harness/, delivery/, reaper/   provider protocol, session and process primitives
  gitgui/, worktree/, isolation/  local filesystem/git/isolation primitives
  transport/, carrier/, relay/   encrypted frames, WebRTC, rendezvous
  changes/                      committed fact invalidation and subscription push
```

`services/` is proposed ownership, not existing code or a request to rename every module at once. Keep the working [harness](../../bridge/src/harness/mod.rs), [store](../../bridge/src/store.rs), [gitgui](../../bridge/src/gitgui.rs), [isolation](../../bridge/src/isolation/mod.rs), and [transport](../../bridge/src/transport.rs) boundaries and the existing `build_bridge::app` facade until callers migrate. The [app composition root](../../bridge/src/app.rs) wires service handles, the store, providers, timers, and event publisher. It contains no workflow decision table after extraction.

| Layer | May import | Must not import |
| --- | --- | --- |
| Transport and API adapters | typed command/query facade, wire/auth primitives | service internals, mutable `AppState` fields, git or provider logic |
| Services | domain records, narrow store repositories, device primitives, fact publisher interface | RPC frames, `serde_json::Value` request shapes, SPA modules |
| Canonical storage and device primitives | domain types and their own low-level dependencies | services, API, event subscribers |
| Event publisher | committed fact IDs, revisions, subscriptions | transition rules or mutation authority |

These are target import rules. Today [RPC routing](../../bridge/src/app/rpc.rs) and [MCP control](../../bridge/src/app/mcp.rs) reach `AppState`, and the [change bus](../../bridge/src/changes.rs) has fact-source closures supplied by app. Preserve behavior while narrowing those interfaces. `app.rs` may import every layer solely for construction and scheduling. Cross-service calls go through typed interfaces or a composition-level coordinator, never a reverse import from a primitive into `app`.

## Service entry points

`services/contracts.rs` holds internal Rust command, query, committed-fact and error types shared by facades; it is not a second serialized client protocol. Wire DTOs and versioned JSON stay in `api/v1` and the MCP adapter. Each service offers a small command facade and query facade. Adapters translate a frame or MCP action once into typed input carrying `Caller`, entity address, operation ID, and expected revision when applicable. A command returns an accepted result or a typed refusal; queries return immutable snapshots or pages. The same service command is used by browser RPC, authenticated MCP, startup recovery, and provider pumps. A provider signal has its own typed input and a verified session instance, not an invented browser caller.

The command facade resolves authorization and exact identity at the trust boundary: browser session/device scope or MCP session token first, then canonical owner, agent, conversation, checkout, and operation receipt. The final service/store mutation rechecks the revision or reservation under the app lock or SQLite transaction. A front-end disabled button, a stale event, or an earlier read is never a guard. Keep [model argv validation](../../bridge/src/models.rs), [MCP token binding](../../bridge/src/app/mcp.rs), scoped paths, and exact session lineage on the device.

`agents` owns the durable turn queue, session claims, model choice at dispatch, usage-limit holds, compaction timing, and resume. It calls the existing [harness interface](../../bridge/src/harness/mod.rs) through an adapter and keeps [delivery receipts](../../bridge/src/app/runtime/delivery/receipts.rs) tied to the accepted operation. `tracker` owns the active [tracking, inbox and reminder](../../bridge/src/app/tracker/mod.rs) rules around workspace agents and conversations. `routing` owns capture disposition and project-agent coordination. `workspaces` owns lifecycle reservations, checkout mutation, and recovery. `notifications` decides from committed attention facts whether a push is due; [signed generic payloads and throttling](../../bridge/src/notify.rs) remain device work. No service waits for a connected SPA.

Issue/Plan creation and scheduling, plus mutating stage and `run.create` commands, are retired. The [RPC retirement guard](../../bridge/src/app/rpc.rs) refuses these mutations before dispatch, while historical document and diff reads remain available; the [old issue scheduler](../../bridge/src/app/issues/scheduler.rs) refuses operation. Keep these compatibility reads and refusals at the API/legacy boundary. Do not extract the retired scheduler into `services/tracker` or recreate its transitions. Existing historical rows and adapters may keep using canonical storage until a separate migration retires them.

## Command and signal flow

```text
User DataChannel frame or local MCP call
  -> E2EE transport decrypt / MCP token check
  -> API adapter parses typed command, caller and operation identity
  -> service authorizes, checks expected revision, reserves if needed
  -> slow device operation off app lock, with owned inputs
  -> service rechecks claim/CAS and commits canonical rows + delivery intent
  -> publisher announces committed fact IDs/revisions
  -> each SPA view rereads its cache after cache update

Provider status, done report, filesystem or timer signal
  -> harness/watch/timer adapter validates source and session generation
  -> same service transition and durable commit path
  -> delivery/notification work proceeds while browser is absent
  -> publisher announces the resulting committed facts
```

The existing [decide/run/apply](../v2/Bridge%20Concurrency%20Primitives.md) rule remains: decide and reserve under a short lock; run blocking git, provider or filesystem work with that lock released; apply only if the claim still matches. The [status pump](../../bridge/src/app/runtime/pumps.rs) already drives deferred turns on provider changes. A service extraction must preserve that producer, not replace it with a browser timer. Session replacement, model change, and resume use exact instance fences in [delivery preflight](../../bridge/src/app/runtime/delivery/preflight.rs) and [resume](../../bridge/src/app/runtime/resume.rs).

For a successful command, commit canonical state, the conversation/operation receipt, and any delivery intent in one SQLite transaction where they are SQLite records. Publish only after the commit. A durable outbox is the target for effects that must survive a crash between commit and send: rows keyed by operation/event identity, claimed and retried by a device publisher, with idempotent consumers. The current [store transaction helper](../../bridge/src/store.rs) and accepted-operation paths in [app transactions](../../bridge/src/app/transactions.rs) are narrower existing seams; there is **no claim that a generic outbox or replay engine exists today**. Existing historical plan files and current worktree mutations are separate filesystem resources, so use their established atomic writes, claims, and recovery choreography rather than claiming a cross-resource SQLite transaction.

Events carry facts after commit: entity ID, kind, revision/cursor, and bounded changed-field hints where safe. They do not carry instructions such as “retry this agent,” “route this capture,” or “notify the user.” The service decides those outcomes before publishing. Invalidation coalescing or truncation requires explicit snapshot reconciliation/reset semantics so a client cannot remain silently stale; querying canonical state is the recovery path, not proof of gap-free delivery. For the #82 render rule, a client writes an arriving fact into its cache first, notifies subscribers second, and every mounted view rereads that cache. Existing [change-bus invalidations](../../bridge/src/changes.rs) remain a compatibility path during migration. A durable outbox is necessary only for effects whose delivery must be recovered, not for making every repaint durable.

## Refactoring sequence and proof

1. Define typed command/query inputs, caller identity, revisions, and service errors beside the existing API. Add adapter tests that browser RPC and MCP resolve the same canonical address and refusal. Keep wire names and response shapes stable.
2. Extract `agents` around the [session registry](../../bridge/src/app/runtime/sessions/registry.rs), [delivery queue](../../bridge/src/app/runtime/delivery/queue.rs), and provider pump. Preserve operation identity, in-flight claims, native receipts, headless usage reset, compaction, and restart recovery. Test races with replaced sessions and disconnected browsers.
3. Extract active `tracker`, `routing`, and `workspaces` commands one vertical slice at a time, moving their rules and persistence together. Leave retired planning mutation guards and historical reads in compatibility adapters. Keep slow work off-lock and verify stale claims cannot settle newer work. Test persisted rows and conversation/receipt atomicity on injected store failure.
4. Introduce post-commit fact emission at each migrated command, then narrow `changes` to publication and subscriptions. Add an outbox only for crash-sensitive delivery/push intents after mapping every current producer and recovery path. Test commit failure emits nothing, restart retains operation identity, and duplicate attempts are deduplicated or remain explicitly uncertain when external delivery cannot be proven.
5. Move notification decisions into the always-on service and leave signed transport as a primitive. Remove old `AppState` workflow methods only when the matching API, MCP, pump, and recovery callers use the service. Run focused Rust tests, the existing concurrency/load gate, wire compatibility tests, and SPA cache-order tests for each slice.

The migration succeeds when adapters contain parsing and trust checks, services contain every persistent decision, storage/device modules have no dependency on API or `AppState`, and event publishers can be replaced without changing a workflow outcome.
