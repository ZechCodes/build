# Plan/Run Split Spec

> **Amendment (2026-09-18):** `post_thread_message` takes only `status`,
> `body` and `options`. There is no `phase`, `outputs`, `anchor` or `links`:
> Build knows which phase a report closes from the session that sent it, and a
> plan's stages are read from `.build/plan/stages.json` on disk when the plan
> agent reports Complete. The per-stage validation gate (validate/fix-stage
> sessions, `ValidationReport`, `run.stage_fix`/`issue.stage_fix`), diff triage
> (`triage.override`, `triage_enabled`, `.build/review-rules.json`), the
> branch-recovery agent (`RecoveryAttempt`, `phase=recover`) and agent-reported
> comment resolutions are removed. A stage is `building` until its build
> reports Complete, then `completed`. Where this document says otherwise, this
> note wins.

Status: **locked** (2026-07-16). Branch: `plan-run-split`.

## Conversation surface amendment (July 29, 2026)

Plans and runs each expose their durable thread as a dedicated, first/default
Conversation tab. Plan artifacts live in Stages; run artifacts live in Stages and
Changes. Threads are no longer appended to either artifact view. The thread wire
model supports validated typed references (`file`, `plan_stage`, `run`) on agent
messages and status events. This amendment supersedes older Review-tab naming but
does not change plan/run ownership, persistence, E2EE boundaries, or comment
delivery semantics.

## Problem

Today a plan cannot exist except as an early phase of a worktree-bound task. `TaskState`
interleaves `Planning → PlanReview → Building → Review` in one lifecycle; `dispatch()`
creates the worktree *before* planning because the plan agent needs a PTY cwd and writes
its docs into `.build/plan/` inside that worktree; the store only mirrors plan docs as
archival history. The envisioned workflow — author plans at the project level, then spin
up worktrees to implement whichever plan you choose — is structurally impossible.

## The split

Two entities replace the fused task.

### Plan (project-scoped)

- Identity: `plan_id`, goal/title, created timestamp.
- Canonical docs live in the **bridge store** at `store/plans/<plan_id>/` — the existing
  archive dir promoted to source of truth. Contents: the stage manifest, stage docs
  (`.build/plan/…` relative layout preserved so materialization is a straight copy), and
  the legacy single `plan.md` shape.
- Carries the stage manifest, per-stage `Planned/Approved` sub-state, and persisted stage
  comments (`StageComment` moves to the plan record).
- Own state machine:

  ```text
  drafting → plan_review → approved
      │  ▲                      │
      │  └── notes (batch) ─────┘   (revision loop; approval of a revised doc is stale)
      └── blocked / failed / idle_unreported / interrupted (same arms as today, Plan phase)
      └── abandoned (terminal)
  ```

- "Implemented" is **derived** from the plan's runs (any run merged), not a stored state.

### Run (worktree-scoped)

- Identity: `run_id`, optional `plan_id` (**a Quick task is a run with `plan_id = None`**),
  worktree, branch, harness/session bookkeeping.
- Build-side lifecycle only:

  ```text
  created → building → review → merged
               │  ▲        │
               │  └ changes┘
               └── blocked / failed / idle_unreported / interrupted (Build phase)
               └── abandoned / archived (terminal; archived = worktree vanished)
  ```

- Multi-stage: the run carries per-stage progress (`Building/Built/Validating/Validated`)
  keyed by the plan's stage ids. The sequential gate (a stage runs only after the previous
  validated) lives on the run. Validation-failed still routes to the stage board.

### Seams

1. **Planning sessions run in disposable worktrees.** Planning needs a PTY, a repo to
   read, and a git diff for enforcement-by-observation. Creating a plan spins up a
   throwaway worktree (same `WorktreeManager`, `plan-<slug>` naming); the agent writes
   `.build/plan/` exactly as today; on `done(phase=plan, completed)` the bridge **ingests**
   the docs into `store/plans/<plan_id>/` (today's `snapshot_plan_docs`, promoted from
   mirror to canonical write) and the worktree is removed. Revision sessions get a fresh
   disposable worktree with the docs re-materialized first.
2. **Dispatching a run materializes the plan.** Run creation copies the plan docs from the
   store into the fresh worktree's `.build/plan/` and commits them, preserving the scope
   doc's "plan is committed and kept through merge" intent record. Build prompts are
   unchanged in shape.
3. **Mid-run revisions write back through the bridge.** Stage-revision sessions
   (comments, validation-failed) run in the run's worktree; the done-ingest updates the
   store copy. **Single-active-writer rule:** at most one active run per plan; a second
   concurrent run of the same plan is rejected at dispatch. (Plan versioning is a later
   extension if parallel runs are ever wanted.)

### What this buys

- Plans authored at project level; implement later, or never, or repeatedly.
- Multiple runs per plan over time (re-attempt after abandon; plan with one harness,
  build with another — the durable-handoff property from the scope doc, now structural).
- Deleting a worktree kills a *run*; the plan was never in it. `Archived` gets honest.

## Protocol & UX

- Project page gets a **Plans rail**: Draft / In review / Approved buckets, plus quiet
  history. "New plan" sits beside "New task" (Quick task remains the plan-less path).
- **Implement** on an approved plan is the trigger that creates worktree + run
  (split-button per the user-agency principle: default "Implement", dropdown for
  harness/model/base-branch choices).
- Plan-review cards (notes, blocked-during-planning) belong to plans; diff-review cards
  belong to runs. Run cards link back to their plan.
- Wire protocol: plan messages (`plan_create`, `plan_notes`, `plan_approve`,
  `plan_abandon`, comment CRUD keyed by `plan_id`) split from run messages (`run_create`
  with optional `plan_id`, `run_changes`, `run_approve_merge`, `run_abandon`, `reply`).
  Board/feed payloads carry both collections. Exact field names follow the existing
  conventions mapped from `app.rs`.

## Persistence & migration

- `PersistedTask` splits into `PersistedPlan` and `PersistedRun` records; a run stores
  `plan_id` instead of plan docs. Store layout: `store/plans/<plan_id>/` (docs + record),
  `store/runs/<run_id>.json` (or matching the existing record layout).
- **Migration on first boot with old records:** each legacy task becomes a plan record
  (from its snapshot docs, comments, manifest) plus — if it progressed past planning — a
  run record pointing at it. Legacy plan snapshots `store/plans/<task_id>` are already in
  the right place; the id is reused as the `plan_id`. No data is deleted; unknown fields
  are preserved where possible. Migration is idempotent and covered by tests against
  fixture stores.

## Non-goals (this branch)

- Plan versioning / parallel active runs of one plan.
- Any change to the E2EE transport, relay, pairing, or terminal surfaces.
- SDK harness integration.

## Implementation decisions (locked after subsystem mapping)

### Domain modules

- New `bridge/src/plan.rs`: `PlanState` (`Created, Drafting, PlanReview, Approved,
  Blocked, Failed, IdleUnreported, Interrupted, Abandoned`), `PlanEvent` (`Dispatch,
  PlanReady, SendNotes, Approve, Blocked, Failed, WentIdle, Interrupt, Reply, Abandon`),
  pure `plan_transition`. Plan-side stage doc state (`Planned/Approved` + `Revised`
  staleness rule), `StageComment`, `CommentAnchor`, manifest types, and the
  `is_worktree_contained_path` fence move here. A plan's coarse state stays `Approved`
  once approved; mid-run doc churn is carried by per-stage doc states only.
- New `bridge/src/run.rs`: `RunState` (`Created, Building, StageGate, Review, Blocked,
  Failed, IdleUnreported, Interrupted, Merged, Abandoned, Archived`), `RunEvent`
  (`Dispatch, BuildReady, RequestChanges, ApproveMerge, Blocked, Failed, WentIdle,
  Interrupt, Reply, Abandon, Archive, ValidationPassed{last_stage}, ValidationFailed`),
  pure `run_transition`. `StageGate` replaces the fused machine's reuse of `PlanReview`
  as the between-stages gate. Run-side per-stage progress: `StageProgress { stage_id,
  state: Building/Built/Validating/Validated{passed}, start_sha, validation }`.
  No `Phase` parameter anywhere — each machine has exactly one working phase.
- `task.rs` shrinks to the legacy deserialization shapes the migration needs, then dies.

### Identity & session routing

- Ids: `plan-<uuid>` / `run-<uuid>`. The MCP CLI stays `mcp --task <id>` (opaque);
  the daemon routes each done report by owner lookup (plans map, then runs map).
  `scaffold_build_dir` writes the owning entity's id and runs for both worktree kinds.
- `DonePhase` stays `plan/build/revise/validate`. Owner kind disambiguates plan-side
  vs run-side `revise`; within a run, `revising_stage_id` bookkeeping disambiguates
  stage-revision (store write-back) from post-review changes, as today.

### Store layout & migration

- `store_dir/plans/<plan_id>/record.json` + `store_dir/plans/<plan_id>/docs/…` (docs
  keep the worktree-relative `.build/plan.md` / `.build/plan/*` layout so
  ingest/materialize are straight copies). `store_dir/runs/<run_id>.json`. Same
  atomic-write + fsync discipline as today.
- `ingest_plan_docs` (worktree → store) is **fail-fast** — an ingest error fails the
  `done` handling; the plan never advances with unpersisted docs. `materialize_plan_docs`
  (store → worktree) is the reverse copy; run-dispatch commits the result.
- Migration on boot, before recovery: each legacy `<task_id>.json` becomes a plan record
  (reusing the task id as plan id; docs from the `plans/<task_id>/` snapshot) and/or a
  run record. Standard task never past planning → plan only. Quick task → run only
  (`plan_id: None`). Past planning → plan (`Approved`) + run (state mapped 1:1;
  fused multi-stage `PlanReview` with any stage progress → `StageGate`). Terminal
  fused states map to terminal run states with the plan kept. Idempotent: successful
  migration renames the legacy file to `<task_id>.json.migrated` (kept, ignored by the
  loader); presence of new-format records short-circuits.

### Worktrees & diffs

- Planning worktrees: branch prefix `plan/<slug>`, created at plan dispatch, **kept warm
  through the notes/revision loop** (the scope doc's warm-session property), torn down
  (worktree + branch) on `Approve` or `Abandon`. Ingest is transactional at every
  plan/revise `done`, so teardown never loses docs. A vanished planning worktree never
  archives a plan — recovery marks the plan `Interrupted`; the next revision dispatch
  re-creates a worktree materialized from the store.
- Run worktrees: branch prefix `build/<slug>` as today. Dispatch order: create worktree →
  scaffold → materialize plan docs → commit ("plan: <goal>") → record that commit as the
  run's `base_sha` → spawn build session. The run's review diff baselines on `base_sha`
  (falls back to merge-base for quick/adopted runs), keeping materialized docs out of
  review noise while still surfacing any build-agent edits to them.
- Planning worktree paths join the bound-paths exclusion set so they never surface as
  adoptable external worktrees; the archive sweep iterates **runs only**.

### Wire protocol (clean cutover; SPA ships in the same branch)

- `plan.create/get/list/doc/stages/stage_doc/approve/send_notes/stage_approve/
  stage_send_notes/comment_add/comment_delete/message/abandon/delete` — plan-scoped,
  keyed by `plan_id`; doc reads come from the canonical store.
- `run.create` (`plan_id` optional; quick runs pass `goal` directly; rejected while the
  plan already has an active run — single-active-writer), `run.get/diff/request_changes/
  stage_dispatch/stage_fix/stage_send_notes/set_auto_advance/git_action/message/abandon/
  delete/adopt/release`. `run.stage_send_notes` is the mid-run revision verb: once a plan
  is Approved, `plan.stage_send_notes` is illegal — the revision session runs in the RUN's
  worktree from the stage gate, and the ingest writes the docs back to the plan's store.
- `board.list` replaces `task.list`: `{ plans: [plan_view], runs: [run_view],
  external_worktrees, primary_changes }`. `run_view` carries `plan_id`; stage progress
  (run) and stage docs/comments (plan) are joined client-side by `plan_id` + stage id.
- `task.approve_merge` and `task.resume` are confirmed dead wire (no SPA references);
  their behavior folds into `run.git_action` / `run.message` recovery paths.

### Model assignment for the build

Fable: domain core (plan.rs/run.rs), store + migration, orchestrator core seams
(dispatch/ingest/materialize/on_done routing/single-active-writer). Opus: orchestrator
periphery, app.rs handlers + recovery + QA sims, templates/diff wiring, all SPA work.

## Invariants to hold through the refactor

- The `done` tool remains the single MCP tool; phase semantics unchanged for agents.
- Quiescence never decides anything; late `done` after idle is honored (both machines).
- Human gates are the definition of done; no auto-advance past `plan_review` or `review`.
- Fenced paths: agent-supplied paths never escape the worktree or the snapshot dir.
- TDD throughout: state machines and migration land test-first.
