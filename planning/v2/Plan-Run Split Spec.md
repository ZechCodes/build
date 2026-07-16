# Plan/Run Split Spec

Status: **locked** (2026-07-16). Branch: `plan-run-split`.

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

## Invariants to hold through the refactor

- The `done` tool remains the single MCP tool; phase semantics unchanged for agents.
- Quiescence never decides anything; late `done` after idle is honored (both machines).
- Human gates are the definition of done; no auto-advance past `plan_review` or `review`.
- Fenced paths: agent-supplied paths never escape the worktree or the snapshot dir.
- TDD throughout: state machines and migration land test-first.
