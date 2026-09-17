# Goal and Task Observation Spec

Status: proposal, 2026-09-17. Documentation only; not an implemented contract.

## Outcome and scope

Build should be able to show an agent's explicit goal objective and status,
its current checklist, and its live execution tasks through one provider-neutral
harness interface. Codex and Claude supply different evidence; the interface
must preserve that difference without requiring provider checks in the SPA.

This proposal extends [Agent Surfaces Spec](Agent%20Surfaces%20Spec.md) and
[Agent Surfaces Primitives](Agent%20Surfaces%20Primitives.md). It retains their
snapshot, bounded-data, read-only, and content-free invalidation rules. The
intentional addition is per-kind observation metadata: an omitted surface
currently cannot distinguish unsupported, not loaded, and known empty.

Reading a goal does not create one. Goal mutation, automatic continuation,
scheduling, task editing, and changes to delivery/interrupt semantics are out
of scope. Build plans, runs, reviewer operations, and conversation topics are
not provider goals and must not populate the goal field.

## Three different kinds of information

| Kind | Meaning | Lifetime |
| --- | --- | --- |
| Goal | Explicit provider-managed objective, status, and usage | Can span many turns |
| Checklist | Agent-declared steps and their reported completion state | Latest list, possibly spanning turns |
| Execution task | A running subagent, shell, or workflow | Can outlive its initiating turn |

`AgentStatus` remains execution status. An active goal with an idle turn is a
valid combination. Turn completion does not complete a goal or its checklist;
a completed checklist does not prove goal achievement. A task description is
not an implicit goal objective. A watcher is not necessarily productive work.

## Carrier capabilities and MVP boundary

The MVP observes only evidence available through the carrier already used to
execute the session. It does not promise feature parity across app-server,
headless, and PTY transports.

| Carrier | Explicit goal | Structured checklist | Execution surfaces | MVP behavior |
| --- | --- | --- | --- | --- |
| Codex app-server | Goal get and notifications | `turn/plan/updated` | Existing subagent surfaces | Goal and checklist supported when the running server confirms them |
| Claude headless SDK | No confirmed top-level goal API | Confirmed `TodoWrite` call/result pairs | Existing SDK task lifecycle surfaces | Goal unsupported; reduce confirmed TodoWrite snapshots |
| PTY carriers | No guaranteed structured API | No guaranteed structured API | Existing carrier-specific surfaces | Leave capability unknown or unsupported from concrete carrier evidence |

Provider-name branching is not capability detection. Concrete session adapters
establish support from their protocol and keep provider differences behind the
harness contract. This MVP makes no durable-restart promise.

## Existing code to extend

- `bridge/src/harness/mod.rs`: `Harness` selects and opens a carrier; it should
  not own live goal state or provider-name capability switches.
- `bridge/src/harness/session.rs`: `AgentSession` already exposes `surfaces()`
  and `surfaces_changed()`, alongside status, activity, and receipts.
- `bridge/src/harness/surfaces.rs`: `AgentSurfaces` contains workflows,
  subagents, shells, and checklist items. `SurfaceRevision` supplies watch
  invalidations; `wire_value` is the shared serializer.
- The Claude surface ledger already reads `TodoWrite`, correlates `TaskCreate`
  calls with returned task IDs, and applies `TaskUpdate` status results. It
  also reads task lifecycle/progress events. Extend these reducers rather than
  introducing a competing task store.
- `bridge/src/harness/codex_app_server/session.rs` already publishes surfaces
  for subagents. Goal reads and checklist notifications are missing. Textual
  `plan` items are deliberately suppressed by the translator; they are distinct
  from structured `turn/plan/updated` checklist notifications.
- Agent detail already forwards surfaces in
  `bridge/src/app/runtime/agents/endpoints.rs`. Preserve this path and the
  existing `entity.changed` refetch mechanism. The revision pump lives in
  `bridge/src/app/runtime/pumps.rs`, detail serialization in
  `bridge/src/app/runtime/sessions/registry.rs`, and surface UI/cache handling in
  `spa/src/core/agentSurfaces{,Model,Render}.js` and `surfacesCache.js`.

## Proposed harness contract

Keep the existing object-safe methods. They return cached data and never wait
for provider IO or hold the application lock while making RPC requests:

```rust
fn surfaces(&self) -> Option<AgentSurfaces>;
fn surfaces_changed(&self) -> Option<watch::Receiver<u64>>;
```

No new `goal()` or `tasks()` trait method is needed: they would duplicate
snapshot/subscription mechanics. Add a goal and per-kind observation metadata
to `AgentSurfaces`; keep existing checklist and execution-task fields. Concrete
provider sessions own RPC correlation, initialization, and resynchronization.
A caller should not have to invoke a refresh to obtain an initial snapshot.

Proposed conceptual types (implementation names may follow local conventions):

```rust
struct SurfaceObservation {
    support: SurfaceSupport, // unknown | supported | unsupported
    freshness: SurfaceFreshness, // loading | current | stale
    coverage: SurfaceCoverage, // complete | partial
    observed_at: Option<String>, // Build receipt time, RFC 3339
}

struct SurfaceGoal {
    objective: String,
    state: GoalState,
    token_budget: Option<u64>,
    tokens_used: Option<u64>,
    time_used_seconds: Option<u64>,
    created_at: Option<i64>, // normalized epoch milliseconds
    updated_at: Option<i64>, // normalized epoch milliseconds
}
```

Add `goal: Option<SurfaceGoal>` plus observation entries for `goal`,
`checklist`, and existing execution surface kinds. Goal states are `active`,
`paused`, `blocked`, `usage_limited`, `budget_limited`, `complete`, and an unknown
state preserving the provider token for diagnostics. Missing usage is unknown,
not zero. Keep elapsed seconds distinct from timestamp milliseconds and verify
provider timestamp units against a captured response before conversion.

Observation rules:

- `supported/current/complete` plus no goal means confirmed no goal. The same
  metadata plus an empty checklist means a confirmed empty list.
- `unsupported` means this carrier cannot supply that kind; freshness and
  coverage carry no claim in that case.
- `unknown/loading` means capability or initial state is not established yet.
- `supported/stale` may retain the last value, but cannot claim it is live.
- `partial` means only a subset was observed, for example after resuming a
  Claude session without a full checklist snapshot.
- Every supported kind has an observation entry even when its value is empty.
  Existing consumers may ignore these additive fields. New consumers replace
  the whole surfaces object on detail refresh, never merge omitted arrays.
- `is_empty()` must account for goal and observation metadata so known-empty
  and unsupported answers are not discarded by serialization/digest gating.

Represent valid observation combinations with constructors or an internal sum
type rather than allowing arbitrary enum combinations. Unknown capability may
be loading or stale/unavailable before the first successful read, with no
coverage claim; unsupported has no freshness or coverage
claim; supported is loading, current, or stale and has complete or partial
coverage where meaningful. Serialization may retain the flat additive wire
shape. Preserve unknown provider state tokens as bounded raw strings.

Proposed design defaults are 4 KiB for a goal objective or checklist item, 1 KiB
for an unknown token or provider ID, 8 KiB for a plan explanation, 256 checklist
items, and 128 retained terminal execution tasks per surface kind. These are
Build safety limits, not provider limits, and may be tuned with fixtures.
Truncation sets partial coverage and reports the omitted count, or the original
count when known; a truncated total must never be presented as complete. Apply
bounds before caching or detail publication. `observed_at` records receipt time
for meaningful accepted evidence. An exact duplicate does not change it, bump
the revision, or create an invalidation loop.

Use typed checklist states (`pending`, `in_progress`, `completed`, `blocked`, unknown)
in new reducer logic, preserving the existing wire tokens. Keep provider task
IDs when present. Index-derived IDs are snapshot-local, never stable identities
for cross-revision timing or mutations. Checklist provenance contains source,
provider session generation, provider turn ID when available, collection epoch,
and whether the snapshot is carried from a prior turn. Turn IDs are opaque; use
observed lifecycle order, never lexical or numeric comparison. A collection
epoch identifies one coherent list. Do not mix `TodoWrite` and `TaskCreate`
evidence into one collection merely because both resemble tasks, and do not
require Claude to invent a turn ID it does not expose.

## Codex: read the explicit goal

The official app-server API provides:

| Method/event | Adapter action |
| --- | --- |
| `thread/goal/get` with `threadId` | Establish current state; response `goal: null` confirms absence |
| `thread/goal/updated` | Replace goal from notification; validate owning thread |
| `thread/goal/cleared` | Clear goal and mark current, complete observation |
| `thread/goal/set`, `thread/goal/clear` | Available upstream, deliberately not invoked in this read-only work |

Verified locally against generated schemas from `codex-cli 0.154.0`: goal
contains `threadId`, `objective`, `status`, optional/null `tokenBudget`,
`tokensUsed`, `timeUsedSeconds`, `createdAt`, and `updatedAt`. Update notifications
also allow a `turnId`. Preserve every goal status; do not collapse blocked,
paused, budget exhaustion, or account usage limits into idle or complete.

After `thread/start` or `thread/resume` establishes the provider thread ID,
enqueue an asynchronous initial goal read. Subscribe before that request can
race with notifications. Add a correlated pending request variant in
`codex_app_server/protocol.rs`; handle response/error in the session reader.
No goal read should open a model turn or alter operation receipts.

Route goal responses, goal notifications, malformed payloads, and goal-read
errors through dedicated Codex handlers before the general execution state
machine. Observation failures update only goal observation metadata and retry
bookkeeping; they must not change `AgentStatus`, finish a turn, create an
operation error, or produce an execution receipt. An omitted `goal` field is a
malformed response, not the same evidence as explicit `goal: null`.

Use a per-thread session generation and goal revision to prevent old replies
from overwriting new state. Capture the revision when issuing a read. If a
notification advances it before the response, retain the newer notification;
if that notification is incomplete or invalid, schedule another read rather
than accepting an old snapshot. Reject responses from previous sessions and
child threads. A clear must not be undone by an older get response.

Probe the actual running server. A method-not-found response marks goal
observation unsupported for that provider session generation. A malformed
success payload marks the observation stale and schedules bounded recovery.
An optional goal-read timeout or transient RPC error marks it stale/unavailable
and permits a bounded retry without failing execution or producing a receipt.
Actual provider transport or process death still follows normal session failure
handling. Do not equate every RPC error with lack of support. If a version
requires an experimental capability, enable it only deliberately with fixtures;
do not infer support solely from the CLI version or generated schema.

Re-read after resume/reconnect and detected event loss. Process updates between
turns, including autonomous continuations. Do not poll on every token or invent
local usage accrual; the provider counters are authoritative.

## Codex: checklist and execution tasks

Consume `turn/plan/updated`: validate `threadId`, record `turnId`, optional
explanation, and ordered `plan` entries. Map `pending`, `inProgress`, and
`completed` to the shared checklist states. Each notification replaces that
turn's list; an explicit empty list clears it. Do not parse narrative plan text
or assistant prose into a checklist.

Retain the latest checklist across turns but mark its origin. A new turn does
not automatically mean the previous list is current work. Switch to the new
turn when it supplies a plan; ignore late older-turn updates for current display.
Use turn-scoped positional IDs where no stable item ID exists.

There is no verified dedicated checklist-get RPC in this investigation.
Following reconnect, recover only from structured history the installed server
actually returns and fixtures prove sufficient. Otherwise retain stale cached
steps or report loading/partial until a fresh plan notification. Do not claim
that `thread/read` reconstructs plan notifications automatically.

Keep existing subagent surfaces and child-thread routing. Child plans/goals
must never overwrite the parent's. Commands and subagent lifecycles populate
execution surfaces, not checklist entries merely because they are running.

## Claude: checklists and execution tasks

The Agent SDK reference inspected exposes no equivalent persistent top-level
goal API. Mark goal unsupported for this carrier; do not fill it from a prompt,
TodoWrite entry, task description, or conversation title. This is a capability
finding for the inspected protocol, not a promise about future Claude releases.

Checklist reduction:

- `TodoWrite`: a fully decoded `todos` array replaces the list, including an
  empty array. Stage the fully decoded input by tool-call ID, pass result
  `is_error` into the reducer, and publish only after the matching successful
  result. Preserve content and normalized state; optional `activeForm` can
  provide current-action wording. Never display a provisional list, implement
  rollback for MVP, or reduce partial streamed JSON. Overlapping writes require
  an issuance/collection-order guard so a delayed older success cannot overwrite
  a later confirmed snapshot. A failed newer write does not suppress an older
  valid success unless a newer successful snapshot has already been confirmed.
- Preserve the existing `TaskCreate`/`TaskUpdate` checklist reducers, with their
  source and collection isolated from TodoWrite checklist snapshots. After
  resume, sparse creates or updates without a known complete base yield partial
  coverage, never a complete checklist.
- Defer `TaskList`/`TaskGet` reconciliation, expanded `TaskUpdate` fields,
  dependencies, and checklist persistence until installed-version fixtures
  establish result shapes and collection semantics. Build must not execute
  model tools just to fetch a checklist.

Keep SDK execution tasks separate from those checklist tools. The MVP preserves
current child-tool isolation through `parent_tool_use_id` and existing routing.
The following richer execution-state corrections are a follow-up:

- `task_started`: ID, description, type, optional background flag and depth.
- `task_progress`: current description/tool, optional summary, tokens, tool
  uses, duration. Summary requires the SDK progress-summary option when used.
- `task_updated`: sparse state patch; merge only supplied fields.
- `task_notification`: terminal outcome and summary, with optional usage.
- `background_tasks_changed`: authoritative live background membership snapshot;
  replace membership instead of pairing starts/finishes, and never infer success
  merely from roster disappearance.

In that follow-up, honor `ambient` where present: exclude ambient watchers/internal work from
productive-work indicators. Unknown ambient state is not proof of productive
work. Preserve task types even if only known types have dedicated viewers.
The background roster is not a checklist snapshot and omits foreground work.
These events update execution status on `ProtocolState.tasks`; descriptive data
remains in the existing `SurfaceLedger`. Do not introduce a third task store.

In the follow-up, the documented repeated initialize/reinitialize flow can supply
background membership on supported SDK versions. Use only if supported by our
carrier. It does not establish checklist completeness. On a fresh process,
reset live membership, mark retained values stale, and use trusted replay or
new provider events to restore knowledge. Do not scrape hidden task files or
terminal paint as an authoritative recovery API.

## Subscription, persistence, and UI

Mutate the provider ledger and snapshot atomically, then bump its surface
revision. A watch can coalesce changes because consumers read the latest whole
snapshot. Subscribe during session open and read an initial snapshot even if no
notification arrives. Surface subscription must be installed and pumped when
the activity receiver is absent; the current activity pump's early return must
not suppress surface changes. Preserve existing session ownership checks.
Duplicate updates should not bump revisions. Keep the session generation in
pump ownership so a replaced session cannot publish into its successor.

The existing client cache key of device/entity/agent is insufficient for these
snapshots. Scope or validate cached surfaces against provider session generation
so replacing a session cannot leak an old goal or checklist into its successor.
Persistence and restore are deferred and must ship together; restored snapshots
remain stale until provider reconciliation. This proposal supersedes the
existing surfaces spec's treatment of a cached snapshot as live for observed
goal/checklist freshness, without redesigning execution-surface freshness in
this MVP. Do not claim durable display until storage and loading exist together.

Use existing entity detail and content-free `entity.changed` notifications.
Never put objective/task text in push invalidations. Bound objective, text,
array sizes, and retained terminal tasks. Report truncation/partial coverage;
do not silently present a truncated checklist count as the complete total.

UI rules:

- Show a compact goal summary and status when known, with an expandable
  checklist and its current step. Combine active goal with current
  execution status for wording: idle with active goal is “Goal active”, not
  “Running”. Blocked/paused/limited states remain visible across turn ends.
- Show completed/total only as checklist progress; never convert that ratio into
  goal percentage. Label a carried list as prior-turn context. Unknown-state
  items stay unknown.
- Claude can show current checklist work without a goal badge. Unsupported
  goal observation must not read “No goal”.
- Stale snapshots show last-known state, never a live pulse. Hide absent
  surfaces while preserving observation metadata for diagnostics.
- Use existing execution surfaces for subagents/shells/workflows. Do not change
  `can_interrupt` based on a goal or unfinished checklist.

## Delivery plan and acceptance checks

1. Add the shared contract, valid-state constructors, proposed bounds, additive
   serialization, and cross-provider fixtures for known-empty, unsupported,
   stale, partial, truncation, and duplicate evidence. Carry metadata through
   detail and digest, make the surface pump independent of activity, and scope
   the client cache to provider session generation.
2. Deliver Codex goal end to end: initial read, dedicated routing, notifications,
   error isolation, surface publication, and compact UI. Fixtures cover every
   status, explicit null/clear, omitted goal, malformed payload, method-not-found,
   transient failure, update-before-read-response, child isolation, and session
   replacement.
3. Add structured checklists: Codex plan replacement and lifecycle ordering,
   then Claude staged TodoWrite confirmation with overlapping-write races. Cover
   explicit empty lists, unknown states, prior-turn provenance, failed results,
   resume partiality, and existing child isolation.
4. Finish the expandable provider-neutral checklist UI. Verify surface-only
   changes refetch detail without transcript rows and stale data never pulses.
5. After carrier fixtures exist, separately add `TaskList`/`TaskGet`
   reconciliation, expanded `TaskUpdate`/dependency handling,
   ambient/background-roster execution corrections, and persistence plus
   restore/reconciliation.

MVP acceptance requires Codex app-server goal and structured-plan observation,
confirmed Claude headless TodoWrite checklists, preserved existing execution
surfaces, observation failures isolated from execution state, initial snapshots
without activity events, replacement-safe cache behavior, and the compact UI.
PTY parity, durable restart, historical reconciliation, and richer Claude task
corrections are follow-up gates, not MVP claims. Retain the original reducer
invariants and race tests when each slice is implemented. Run targeted Rust and
SPA tests for implemented slices; no test run is required for this planning
document alone.

Completion means callers can read goal and task state solely through
`AgentSession` surfaces; neither orchestrator nor SPA parses provider events,
infers goals from prose, or mistakes absent evidence for completed work.

## Evidence and remaining verification

- [Codex app-server goal API](https://learn.chatgpt.com/docs/app-server#manage-a-thread-goal)
  and local generated 0.154.0 schemas establish the goal methods and fields.
- [Claude Agent SDK TypeScript reference](https://code.claude.com/docs/en/agent-sdk/typescript)
  establishes task lifecycle, progress, and background roster events. Some fields
  are version-dependent; implementation must fixture the carrier we actually run.
- t3code source inspected at `d4d5d12e8ba086cfbf79ca3adeb4156b46ead665` has
  generated goal bindings but no application goal mapping. Its
  [shared checklist UI](https://github.com/pingdotgg/t3code/blob/d4d5d12e8ba086cfbf79ca3adeb4156b46ead665/apps/web/src/components/ChatView.tsx#L5847)
  consumes plan progress, including Claude TodoWrite translated into plan events.
  That is a checklist precedent, not evidence of an autonomous-goal indicator.
- Before implementation, capture goal timestamp units, permission/experimental
  errors on supported Codex versions, and Claude TaskList/Get/update result
  shapes. Unsupported recovery stays explicit until those fixtures exist.
