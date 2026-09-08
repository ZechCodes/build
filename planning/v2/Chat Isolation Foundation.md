# Chat Isolation Foundation — Implementation Contract and Verification Tracker

**Status: implemented and verified.** This document records
the chat-isolation foundation and its verification evidence. It does not replace
the binding work-isolation specifications.

## Canonical execution target

A dispatched operation resolves its execution context once, as the explicit
triple `(entity_id, agent_id, conversation_id)`.

- `entity_id` identifies the issue/run/entity the operation is about.
- `agent_id` identifies the durable agent that owns its configuration and work.
- `conversation_id` identifies the exact transcript to which execution events,
  receipts, and messages belong.
- All lifecycle, delivery, resume, and UI paths must carry the resolved triple;
  they must not recover any member by selecting a current, default, or newest
  record later.

The triple is context, not an inferred relationship. A caller lacking one member
must resolve it through an explicit addressed lookup or refuse the operation.

## Durable ownership and transcript history

Agent-owned settings are durable and addressed by `agent_id`:

- `choice_revision` is monotonically advanced whenever the agent's effective
  choice changes.
- A new post validates its captured settings revision before acceptance. A retry
  of an already accepted operation retrieves the original receipt first, even
  if the agent's settings have since changed.
- `working_time` is exposed per agent execution, never as a global mutable
  timer. A derived duration need not itself be persisted.

Transcript/history paging is separate from chat-controller addressing:

- paging is addressed by the canonical `conversation_id` and may only retrieve
  that conversation's transcript history;
- it cannot select an agent, entity, or conversation for a mutating controller.

A separate addressed chat controller owns provisional drafts, model selection,
and execution. Its operations use explicit live `(entity_id, agent_id,
conversation_id)` scope rather than inferring it from transcript history.

## Session identity and resumption

Every session instance records exact lineage: its originating execution context,
the session-instance identity, and the checkout/workspace it was assigned.

- Resume uses that exact instance lineage and its recorded checkout.
- “Newest session,” newest conversation, or newest current working directory is
  never a valid substitute for the recorded destination.
- A fresh session's identity must come from its launch contract or the provider
  protocol. Seeing a single new transcript file in a shared checkout is not
  sufficient: another agent may simply have written its file first.
- A missing, mismatched, or no-longer-authorized instance does not bind to
  another session. It starts a fresh session using the canonical context and
  catches up from that conversation's history.
- Providers apply immutable per-turn settings when supported. Otherwise a
  settings change waits for a safe session boundary. Clearing sticky overrides
  to provider Default requires a fresh, non-resumed session, not a resume that
  silently retains the previous override.

## Draft and send contract

An existing agent's composition belongs to its addressed controller. Opening
the new-agent composer creates a separate provisional draft; it never aliases
the currently selected agent's input. Successful creation binds that provisional
controller once, with an idempotent `creation_id` for response-loss recovery.

Send captures an immutable envelope before handoff:

`{ operation_id, entity_id, agent_id, conversation_id, choice_revision, body }`.

- The send always addresses that captured original destination.
- A later agent-choice update cannot retarget an accepted send.
- A result applies only when its captured revision still governs that operation;
  no older completion may overwrite a newer choice or draft state.
- UI reconciliation must be idempotent by `operation_id`.

The execution envelope includes the exact accepted message payload, not merely
a notification to consume every unread message. This distinction matters when
two messages queue under different model choices. Each operation carries its
own normalized messages (including attachments, anchors, and option replies)
and bounded prior context. A later post must not enter an earlier turn's
catch-up or be consumed under that earlier turn's settings.

Operation-owned messages retain their provenance in the canonical transcript.
An operation-addressed provider read validates the execution agent and canonical
conversation and acknowledges only those messages. Unscoped legacy unread
reads cannot consume another queued operation's messages. Provider queue
acceptance alone does not mark a message seen.

## Receipt and handoff semantics

New clients must provide an `operation_id`, durable and unique at intent
creation. It is optional only for defined legacy compatibility. The version-one
receipt feature is `thread_post_operations`; status is read through
`thread.operation`. A post returns `operation_status`; a status query returns
`status`. The transcript append, receipt, and delivery intent commit atomically
before provider handoff. Acceptance is not provider completion.

| Outcome | Meaning | Required UI/state behavior |
| --- | --- | --- |
| Queued | Bridge acceptance and a durable receipt are confirmed. Provider handoff is not implied. | Show queued; retries must reuse `operation_id`. |
| Claimed | The bridge durably reserved provider handoff. | Do not issue another delivery attempt. |
| Delivered | The provider-facing handoff completed. Agent work may still be running. | Reconcile the optimistic message to its durable sequence. |
| Uncertain | Caller cannot establish whether handoff occurred. | Preserve the intent as uncertain; never manufacture a second send. |
| Rejected | Handoff did not accept the intent. | Keep the draft/retry path with the same explicit target. |

An uncertain operation may safely retry the *same* `operation_id` to reconcile
bridge acceptance; it must not blindly replay uncertain provider execution.
Restart recovers queued intents. A claim left behind by a crash becomes
uncertain rather than being blindly replayed. This is not an exactly-once
guarantee about an external provider; transcript insertion and receipt lookup
are idempotent within the bridge.

## Shared transcripts and removal

Issue and run views intentionally share a transcript when their explicit
conversation linkage says they do. Sharing is not an accidental “latest chat”
alias.

An issue displaying implementation work exposes an explicit execution context.
Its composer, model picker, and timer must address that same implementation
agent, while the history retains the issue's canonical conversation identity.
Shared history alone never selects the executor. Legacy omitted-agent calls
may retain their documented compatibility routing; explicit agents do not.

- Removing a primary entity must not rebind remaining controllers, sessions,
  drafts, or sends to another entity/conversation.
- Existing linked transcript history remains addressable according to retention
  policy; a removed primary's absence is explicit.
- Any replacement linkage is a separately recorded, deliberate migration with
  auditability—not fallback resolution.
- Legacy migration preserves the then-current first agent's effective binding
  once, even if an earlier primary was already removed. After that migration,
  persisted bindings are fixed and roster position cannot change them.

## Implemented layers

1. **Model and storage:** add canonical context, durable agent settings,
   session-instance lineage, operation receipts, and migration readers/writers.
2. **Addressed services:** require the triple at lifecycle, delivery, resume,
   and transcript boundaries; remove newest/current-cwd fallback paths.
3. **SPA state:** separate transcript history from controller scope; implement
   provisional draft, captured send envelopes, revision-aware reconciliation,
   and queued/uncertain presentation.
4. **Compatibility migration:** derive each agent's initial durable choice from
   its *current effective choice* (including inheritance/default resolution),
   not a stale legacy field. Backfill only defensible lineage; mark unknowns
   unresolved rather than guessing.
5. **Capability/version migration:** expose and test the versioned receipt
   capability, retain only the defined legacy `operation_id` compatibility, and
   remove fallback behavior when the addressed contract is available.

The bridge owns durable agent identity/settings, exact session lineage, and
transactional operation receipts. Harnesses implement the per-turn settings
capability: native application when supported, or a safe fresh-session boundary.
The SPA uses an account/device-scoped `ChatRepository`, private addressed
controllers, a revision-aware choice controller, and entity adapters. Transcript
caches may be intentionally shared by canonical conversation; drafts, choices,
send reconciliation, and execution remain independently addressed.

## Contract verification

- [x] Storage tests prove the context triple, `choice_revision`, `working_time`,
  session lineage, and `operation_id` survive restart and migration.
- [x] Service tests reject invalid explicit/mismatched context and prove no newest-record
  or current-working-directory resume path is reachable; unavailable exact
  instances start fresh with canonical transcript catch-up. Defined legacy
  omitted-address calls resolve once at the boundary.
- [x] Concurrency tests prove an older send/result cannot overwrite a newer
  revision, draft, or destination.
- [x] Delivery fault tests distinguish bridge-accepted queued from uncertain,
  prove same-`operation_id` reconciliation does not replay provider execution,
  and keep transcript effects idempotent.
- [x] SPA tests prove transcript paging cannot change addressed chat-controller
  scope for drafts, model selection, or execution.
- [x] Removal tests prove no primary-removal rebinding and intentional shared
  issue/run transcript behavior.
- [x] Migration fixtures cover inherited/current effective choices and stale
  legacy values; unknown lineage remains explicitly unresolved.
- [x] Versioned capability tests cover `thread_post_operations`/`thread.operation`,
  required new-client IDs, legacy compatibility, restart, reconnect, duplicate
  delivery, and exact session resume against the original checkout.

The final bridge suite passed 2,129 tests (seven intentionally ignored), including
2,053 library tests. Formatting, strict Clippy, and test compilation checks
passed. The SPA passed all 3,363 tests across 194 files, lint, and production
build. Independent scoped implementation review approved the final contracts.
The companion security checklist records the full functional and scanner gates,
their limitations, and the ignored opt-in fixtures. No deployment or daemon
restart was performed as part of this implementation.
