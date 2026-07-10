# Multi-Stage Planning — Binding Technical Spec

**Status:** Binding. Implementers follow this exactly; names and shapes below are the
contract between six sequential implementation layers. Where this spec pins a name,
use that name. Where behavior is unspecified, match the existing code's conventions
(fail fast, pure domain core, `#[serde(default)]` forward-compat, TDD).

**Feature summary.** Planning produces an ordered set of *stage documents* under
`.build/plan/` with a manifest. Stages build sequentially in the task's single
worktree/branch; after each stage build an automatic *validation agent* gates the
next stage. Plan comments become per-stage, structured, persisted server-side.
Each stage is individually approvable and dispatchable; an `auto_advance` flag
("run all") auto-dispatches the next approved stage when validation passes.
Legacy single-plan tasks and Quick tasks are untouched.

---

## 0. Global decisions (read first)

1. **`Phase` stays 2-valued** (`Plan | Build`). Justification: widening `Phase` (or
   adding a `TaskState::Validating`) ripples through every `TaskState` match —
   `needs_attention`, `is_working`, `state_str`, board buckets, SPA chips, store
   round-trip tests — for information the stage record already carries. Validation
   is an agent working on the Build side of the gate, so a validation session that
   blocks/fails/idles/interrupts lands in `Blocked(Build)` / `Failed(Build)` /
   `IdleUnreported(Build)` / `Interrupted(Build)`, and recovery routing uses the
   persisted **stage sub-state** (`Building` vs `Built`/`Validating`) to decide
   which session to respawn. Zero changes to the `TaskState` enum.
2. **No new `TaskState` variants.** The between-stages gate ("stage N validated,
   stage N+1 not yet dispatched" — pass *or* fail) is `TaskState::PlanReview`: the
   plan tab is the stage board, and `PlanReview` already means "the plan surface
   needs the human" (needs_attention = true, push notification fires). The stage
   records distinguish pass from fail for the UI.
3. **Stage builds reuse `phase: "build"`** in the `done` tool (no `build_stage`
   phase). The bridge already routes `done` contextually (`revise` maps to
   `BuildReady` today); the stage identity comes from the bridge's own
   `current_stage_id`, never from the agent, so a self-describing phase value
   would add schema surface without adding trust. Fewer enum values = fewer agent
   mistakes.
4. **Legacy tasks stay on the legacy path.** A task is multi-stage **iff** its
   `stages` vec is non-empty. Old persisted `plan_path`-only tasks load with
   `stages: []` and keep every existing code path (`task.plan`,
   `task.approve_plan`, `task.send_notes`). No implicit-single-stage migration:
   synthesizing a stage state from a mid-flight `Blocked(Build)`/`Interrupted`
   task is ambiguous, and the legacy path stays exercised by Quick tasks anyway.
5. **Multi-stage is the new default for Standard tasks.** The rewritten `plan`
   template instructs the planner to emit a manifest (a small goal legitimately
   yields a one-stage manifest). Quick tasks are unchanged (no plan phase, no
   stages). The QA scripted agent mirrors production: Standard tasks get a
   deterministic 2-stage manifest (§9.5).
6. **Auto-advance never skips approval.** It dispatches the next stage only if
   that stage is already `Approved`; otherwise the task waits at `PlanReview`
   with `auto_advance` still true. On validation **failure** (or a blocked/failed
   `done` from any stage/validation session) `auto_advance` flips to `false`.

---

## 1. Stage domain model (`bridge/src/task.rs`)

All new types live in `task.rs` (the pure domain core — no IO). `mcp.rs`,
`store.rs`, `orchestrator.rs`, and `app.rs` import them from there.

### 1.1 New types

```rust
/// Position of one stage in its per-stage lifecycle. The task-level state stays
/// coarse; this is the sub-state the stage carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    /// The stage doc exists in the manifest; not yet approved by the human.
    Planned,
    /// The human approved this stage's doc.
    Approved,
    /// A build (or fix) session is running for this stage.
    Building,
    /// The build session reported done; validation has not started yet.
    Built,
    /// A validation agent session is running for this stage.
    Validating,
    /// Validation reported. `passed: true` is terminal for the stage;
    /// `passed: false` awaits `Dispatch` (a fix session) or a plan change.
    Validated { passed: bool },
}

/// Everything that can drive a stage transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageEvent {
    /// The human approves the stage doc. Planned → Approved.
    Approve,
    /// A build session is spawned for this stage. Approved → Building;
    /// Validated{passed:false} → Building (the fix path).
    Dispatch,
    /// The stage's build/fix session reported done(completed). Building → Built.
    BuildDone,
    /// The validation session is spawned. Built → Validating.
    StartValidation,
    /// The validation session reported done(completed). Validating → Validated.
    ValidationDone { passed: bool },
    /// A plan-revision session completed for this stage; the doc changed, so any
    /// approval is stale. Planned → Planned; Approved → Planned.
    Revised,
}

/// A rejected stage transition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal stage transition: {event:?} is not valid from {from:?}")]
pub struct IllegalStageTransition {
    pub from: StageState,
    pub event: StageEvent,
}

/// The pure stage transition function — same discipline as `transition`.
pub fn stage_transition(
    state: &StageState,
    event: StageEvent,
) -> Result<StageState, IllegalStageTransition>
```

Exact transition table (everything else is `Err(IllegalStageTransition)`):

| from | event | to |
|---|---|---|
| `Planned` | `Approve` | `Approved` |
| `Planned` | `Revised` | `Planned` |
| `Approved` | `Revised` | `Planned` |
| `Approved` | `Dispatch` | `Building` |
| `Validated{passed:false}` | `Dispatch` | `Building` |
| `Building` | `BuildDone` | `Built` |
| `Built` | `StartValidation` | `Validating` |
| `Validating` | `ValidationDone{passed}` | `Validated{passed}` |

`Validated{passed:true}` accepts no events (stage-terminal).

```rust
/// One entry of the plan manifest as the agent reports it (`.build/plan/stages.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageManifestEntry {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
}

/// The validation agent's verdict for one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub passed: bool,
    /// Markdown findings — what matched/diverged from the stage doc.
    pub findings: String,
    /// Markdown notes handed to the next stage's build prompt (and surfaced on
    /// the next stage in the UI). Empty string when there is nothing to say.
    pub notes_for_next_stage: String,
}

/// One stage: manifest data + lifecycle sub-state + validation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
    pub state: StageState,
    /// `git rev-parse HEAD` of the worktree at the moment this stage was first
    /// dispatched — the base of "the diff this stage produced". Kept across fix
    /// re-dispatches so the stage diff always covers all of the stage's work.
    #[serde(default)]
    pub start_sha: Option<String>,
    #[serde(default)]
    pub validation: Option<ValidationReport>,
}

impl Stage {
    /// A freshly planned stage from a manifest entry.
    pub fn from_manifest(entry: StageManifestEntry) -> Stage // state: Planned, start_sha/validation: None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentState { Open, Addressed }

/// Where a plan comment anchors inside a stage doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchor {
    /// The chain of enclosing heading *texts* (raw markdown text, outermost
    /// first), e.g. ["Database schema", "Tables"]. Empty for a top-of-doc anchor.
    pub heading_path: Vec<String>,
    /// The selected passage, trimmed, capped at 400 chars by the producer.
    pub snippet: String,
}

/// One persisted, structured plan-review comment on a stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageComment {
    /// Bridge-minted: "c-<n>", n = 1 + max numeric suffix among the task's
    /// existing comment ids (so ids never collide after deletes).
    pub id: String,
    pub stage_id: String,
    /// None = a general comment on the stage (no text anchor).
    #[serde(default)]
    pub anchor: Option<CommentAnchor>,
    pub body: String,
    pub state: CommentState,
    #[serde(default)]
    pub agent_reply: Option<String>,
}
```

### 1.2 `TaskEvent` additions

```rust
pub enum TaskEvent {
    // ... existing variants unchanged ...
    /// The validation agent reported done(phase=validate, completed, passed=true).
    /// `last_stage` = the validated stage is the manifest's final stage.
    ValidationPassed { last_stage: bool },
    /// done(phase=validate, completed, passed=false).
    ValidationFailed,
}
```

`TaskEvent` keeps `Copy` (a bool struct variant is `Copy`).

New arms in `transition` (everything else unchanged):

| from | event | to |
|---|---|---|
| `Building` | `ValidationPassed{last_stage:true}` | `Review` |
| `Building` | `ValidationPassed{last_stage:false}` | `PlanReview` |
| `Building` | `ValidationFailed` | `PlanReview` |
| `IdleUnreported(Build)` | `ValidationPassed{last_stage:true}` | `Review` |
| `IdleUnreported(Build)` | `ValidationPassed{last_stage:false}` | `PlanReview` |
| `IdleUnreported(Build)` | `ValidationFailed` | `PlanReview` |

(The `IdleUnreported` arms preserve the existing rule: quiescence never decided
anything; a late `done` is still honored.)

**Event reuse (no new events needed for these):**
- Dispatching a stage (first dispatch or a later one from the between-stages gate)
  is `TaskEvent::ApprovePlan` — the existing `PlanReview → Building` edge.
- Sending per-stage plan notes is `TaskEvent::SendNotes` — `PlanReview → Planning`
  (also legal from `Interrupted(Plan)`, unchanged).
- A stage build session finishing does **not** raise a task-level event: the task
  stays `Building` while validation runs (the stage moves `Building → Built →
  Validating`). Only validation's outcome moves the coarse state.

### 1.3 Coarse-state ↔ stage-state coherence

While the task is `Blocked(Build)` / `Failed(Build)` / `IdleUnreported(Build)` /
`Interrupted(Build)`, the current stage keeps the sub-state it had when the
session was live (`Building` or `Validating`/`Built`) — the pair routes recovery:

| persisted `TaskState` | multi-stage? | current stage state | `task.resume` respawns |
|---|---|---|---|
| `Interrupted(Plan)` | yes, `revising_stage_id = None` | — | `plan` template (initial multi-stage planning) |
| `Interrupted(Plan)` | yes, `revising_stage_id = Some(id)` | — | `revise_stage` template for that stage with its open comments |
| `Interrupted(Build)` | yes | `Building`, `validation` is `Some({passed:false, ..})` | `fix_stage` template (it was a fix session; findings come from the stored report) |
| `Interrupted(Build)` | yes | `Building`, otherwise | `build_stage` template |
| `Interrupted(Build)` | yes | `Built` or `Validating` | `validate` template (stage state forced to `Validating` if it was `Built`) |
| any | no (legacy) | — | existing behavior, unchanged |

Boot recovery itself (app.rs `recover_task`) is unchanged: a working coarse state
becomes `Interrupted(phase)`; the stage vec rides along untouched.

### 1.4 Tracking the current stage

- `current_stage_id: Option<String>` — the stage whose build/fix/validate session
  is (or was last) in flight. Set on every stage dispatch; left in place after
  validation (the UI and merge review use it); meaningless for legacy tasks.
- `revising_stage_id: Option<String>` — set when a per-stage plan-revision session
  is spawned (`task.stage_send_notes`), cleared when the revise session's `done`
  is consumed. Routes `Interrupted(Plan)` recovery.

Both live on `ActiveTask` (orchestrator) and in `PersistedTask` (§5).

---

## 2. Plan artifact contract

### 2.1 Manifest: `.build/plan/stages.json`

A **top-level JSON array**, order = execution order. Exact shape:

```json
[
  {
    "id": "database-schema",
    "title": "Database schema",
    "path": ".build/plan/01-database-schema.md",
    "summary": "Create the tables and the migration."
  },
  {
    "id": "api-endpoints",
    "title": "API endpoints",
    "path": ".build/plan/02-api-endpoints.md",
    "summary": "CRUD routes over the new tables."
  }
]
```

- `id`: a stable kebab-case slug, `^[a-z0-9]+(-[a-z0-9]+)*$`, unique within the
  manifest, ≤ 48 chars. **Stable across revisions**: a revision that retitles a
  stage keeps its `id`.
- `title`: human heading, non-empty.
- `path`: worktree-relative, must start with `.build/plan/`.
- `summary`: 1–2 sentences; may be empty.

### 2.2 Stage docs

Naming convention (a default the template instructs, not a hardcode — `path` in
the manifest is authoritative): `.build/plan/<NN>-<id>.md` where `NN` is the
1-based, zero-padded two-digit manifest position at creation time. Docs are **not
renamed** by later revisions even if order shifts; only the manifest array order
defines execution order.

New constants in `templates.rs`:

```rust
pub const STAGES_DIR: &str = ".build/plan";
pub const STAGES_MANIFEST_PATH: &str = ".build/plan/stages.json";
```

(`DEFAULT_PLAN_PATH = ".build/plan.md"` stays, for legacy/Quick.)

### 2.3 Stable-id rules across revisions

- The initial plan session creates the manifest and all docs.
- A **per-stage revision** (`revise_stage`) may edit only its stage's doc and its
  own entry's `summary`/`title` in `stages.json`. It must not add, remove, or
  reorder stages, and must not change any `id` or `path`. (Instructed by the
  template; enforcement is by observation — the diff shows violations.)
- There is no full-manifest restructure flow in this feature. If the user wants a
  different stage structure, they abandon or revise per-stage.

### 2.4 Legacy coexistence

`.build/plan.md` remains the plan artifact for legacy persisted tasks. A
multi-stage plan `done` reports `outputs.plan_path = ".build/plan/stages.json"`
(satisfying the existing plan_path requirement) **plus** `outputs.stages` (the
manifest echo). The wire echo — schema-validated by the MCP server — is what the
orchestrator uses to initialize stage records; the file on disk is the artifact
agents and humans read. `task.stage_doc` reads docs from the worktree on demand,
exactly as `task.plan` reads the legacy file today.

---

## 3. `done` tool schema v2 (`bridge/src/mcp.rs`)

### 3.1 Types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DonePhase { Plan, Build, Revise, Validate }   // Validate is new

/// One per-comment resolution from a stage plan-revision session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentResolution {
    pub comment_id: String,
    pub response: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneOutputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
    /// Echo of .build/plan/stages.json. Presence of a non-empty array on
    /// phase=plan/completed marks the task multi-stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<crate::task::StageManifestEntry>>,
    /// Required when phase=validate and status=completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::task::ValidationReport>,
    /// Optional on phase=revise/completed: per-comment resolutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_resolutions: Option<Vec<CommentResolution>>,
}
```

`DoneError` gains:

```rust
#[error("outputs.validation is required when phase=validate and status=completed")]
MissingValidationReport,
#[error("invalid outputs.stages: {0}")]
InvalidStages(String),
```

### 3.2 Validation rules (`DoneReport::from_args`)

| phase | status | rule |
|---|---|---|
| plan | completed | `plan_path` required (unchanged). If `stages` is present: non-empty, every `id` matches `^[a-z0-9]+(-[a-z0-9]+)*$`, ids unique, every `path` starts with `.build/plan/`, every `title` non-empty — else `InvalidStages` naming the first offense. |
| plan | blocked/failed | no outputs required (unchanged). |
| build | any | no outputs required (unchanged). Stage identity is bridge-side context, never agent-supplied. |
| revise | completed | `comment_resolutions` optional; entries with empty `comment_id` → `InvalidStages`-style rejection is **not** applied — unknown/empty ids are ignored downstream by the orchestrator (the MCP server cannot know valid ids). |
| validate | completed | `validation` required, else `MissingValidationReport`. |
| validate | blocked/failed | no outputs required. |
| any | any | `stages`/`validation` supplied on phases that don't consume them are accepted and ignored. |

### 3.3 Exact tool input JSON Schema (`done_input_schema`)

```json
{
  "type": "object",
  "properties": {
    "phase": { "type": "string", "enum": ["plan", "build", "revise", "validate"] },
    "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
    "summary": { "type": "string", "description": "A short markdown summary for the human reviewer. Lead with a one-line outcome, then a few '- ' bullet points of the key changes — or, if blocked/failed, what is needed to proceed. Use markdown: bullets, **bold**, and `backticks` for paths and commands. Prefer scannable bullets over one long paragraph." },
    "outputs": {
      "type": "object",
      "properties": {
        "plan_path": { "type": "string", "description": "Required when phase=plan and status=completed. For a multi-stage plan, the manifest path .build/plan/stages.json." },
        "stages": {
          "type": "array",
          "description": "Echo of .build/plan/stages.json, in execution order. Required when phase=plan, status=completed and the plan is multi-stage.",
          "items": {
            "type": "object",
            "properties": {
              "id": { "type": "string", "description": "Stable kebab-case slug; never changes across revisions." },
              "title": { "type": "string" },
              "path": { "type": "string", "description": "Worktree-relative, under .build/plan/." },
              "summary": { "type": "string" }
            },
            "required": ["id", "title", "path"]
          }
        },
        "validation": {
          "type": "object",
          "description": "Required when phase=validate and status=completed.",
          "properties": {
            "passed": { "type": "boolean" },
            "findings": { "type": "string", "description": "Markdown: what the diff did and did not satisfy from the stage doc." },
            "notes_for_next_stage": { "type": "string", "description": "Markdown notes the next stage's builder should know. Empty string if none." }
          },
          "required": ["passed", "findings", "notes_for_next_stage"]
        },
        "comment_resolutions": {
          "type": "array",
          "description": "When phase=revise and the prompt listed [c-N] comment ids: one entry per comment saying how it was addressed.",
          "items": {
            "type": "object",
            "properties": {
              "comment_id": { "type": "string" },
              "response": { "type": "string" }
            },
            "required": ["comment_id", "response"]
          }
        }
      }
    }
  },
  "required": ["phase", "status", "summary"]
}
```

The `done` tool's top-level description is unchanged. The socket forwarding in
`main.rs` (`{"task_id": ..., "report": ...}`) needs no change — `DoneOutputs`
round-trips through serde with the new optional fields.

---

## 4. Templates (`bridge/src/templates.rs`)

### 4.1 `Vars` additions

```rust
#[derive(Debug, Default, Clone)]
pub struct Vars<'a> {
    pub goal: &'a str,
    pub plan_path: &'a str,
    pub comments: &'a str,
    pub base_branch: &'a str,
    // new — all default to "" and render as empty:
    pub stage_id: &'a str,
    pub stage_title: &'a str,
    pub stage_path: &'a str,
    pub stage_summary: &'a str,
    /// The next stage's doc path, or "" when validating the final stage.
    pub next_stage_path: &'a str,
    /// git sha of the worktree HEAD when the stage was first dispatched.
    pub stage_start_sha: &'a str,
    /// Validation findings, for the fix_stage template.
    pub findings: &'a str,
    /// notes_for_next_stage from the previous stage's validation report.
    pub prior_notes: &'a str,
}
```

`render` gains the corresponding `.replace("{stage_id}", ...)` etc. for:
`{stage_id}`, `{stage_title}`, `{stage_path}`, `{stage_summary}`,
`{next_stage_path}`, `{stage_start_sha}`, `{findings}`, `{prior_notes}`.

### 4.2 `Templates` struct

```rust
pub struct Templates {
    pub plan: String,            // REWRITTEN (multi-stage)
    pub build: String,           // unchanged (Quick + legacy)
    pub build_stage: String,     // NEW
    pub revise: String,          // unchanged (legacy plan revise)
    pub revise_stage: String,    // NEW
    pub fix_stage: String,       // NEW
    pub review_changes: String,  // unchanged (diff comments)
    pub validate: String,        // NEW
}
```

The override mechanism is the struct itself (clone-and-edit per project); new
templates are plain new fields, so nothing about the mechanism changes. If/when a
file loader lands, the file names are the field names:
`.build/templates/{plan,build,build_stage,revise,revise_stage,fix_stage,review_changes,validate}.md`.

### 4.3 Full template texts

`PLAN` (replaces the current one; note it still satisfies the "markdown/bullet"
assertions in the existing tests):

```text
You are in PLAN mode. The goal is:

{goal}

Break the work into sequential stages and write one self-contained markdown plan
document per stage under `.build/plan/`, named `NN-<stage-id>.md` (`01-`, `02-`, …).
Also write the manifest `.build/plan/stages.json`: a JSON array, in execution
order, of {"id", "title", "path", "summary"} — `id` is a stable kebab-case slug
that must never change once written. Use as few stages as the goal honestly needs
(one is fine for small goals); each stage must leave the codebase working, and a
cold agent with no memory of this conversation must be able to execute any single
stage document from scratch given only the previous stages' commits. Plan only —
do not implement anything. Write nothing outside `.build/`.

When the plan is ready, call the `done` tool with phase="plan", status="completed",
outputs.plan_path=".build/plan/stages.json", outputs.stages set to the exact
contents of the manifest, and a short markdown summary for the reviewer: a
one-line outcome, then one `-` bullet per stage. Use `backticks` for paths and
commands; prefer scannable bullets over one long paragraph. If you cannot proceed,
call `done` with status="blocked" and, in the same markdown format, say what you need.
```

`BUILD_STAGE`:

```text
Execute ONE stage of a multi-stage implementation plan. The overall goal is:

{goal}

Your stage is "{stage_title}" — its plan document is at {stage_path}. Earlier
stages are already implemented in this worktree (branched from {base_branch});
later stages will be built by other agents afterwards, so implement this stage
only. Notes from the previous stage's validation:

{prior_notes}

When this stage's work is complete, call the `done` tool with phase="build",
status="completed", and a short markdown summary for the reviewer: a one-line
outcome, then a few `-` bullets of the key changes (use `backticks` for paths and
commands). If you get stuck, call `done` with status="blocked" (you need
something) or status="failed" (the approach did not work) and, in the same
markdown format, say what is needed to proceed.
```

`REVISE_STAGE`:

```text
The reviewer left comments on the plan document for stage "{stage_title}" at
{stage_path}:

{comments}

Revise that stage document to address every comment. You may also update this
stage's "title" and "summary" fields in `.build/plan/stages.json`, but do not
add, remove, reorder, or re-id stages, and do not touch other stages' documents.
Keep writing only inside `.build/`. When done, call the `done` tool with
phase="revise", status="completed", outputs.comment_resolutions set to one
{"comment_id", "response"} entry per [c-N] comment above saying how you addressed
it, and a short markdown summary (a one-line outcome, then `-` bullets of what changed).
```

`FIX_STAGE`:

```text
An automated validation pass reviewed stage "{stage_title}" (plan document at
{stage_path}) against the changes it produced, and it did not pass. Findings:

{findings}

Reviewer note (may be empty):

{comments}

Address every finding in this worktree — the stage's earlier work is your
starting point (`git diff {stage_start_sha}` shows everything this stage has
changed so far). Implement this stage only. When done, call the `done` tool with
phase="build", status="completed", and a short markdown summary of what changed
(a one-line outcome, then `-` bullets). If you get stuck, call `done` with
status="blocked" or status="failed" and say, in the same markdown format, what is
needed to proceed.
```

`VALIDATE`:

```text
You are a VALIDATION agent. Stage "{stage_title}" of a multi-stage plan was just
built in this worktree. Do not modify any files — observe and report only.

Read the stage's plan document at {stage_path}, then examine exactly what the
stage changed with `git diff {stage_start_sha}` (plus any commands you need to
inspect the result, e.g. running the project's tests). Then read the NEXT stage's
plan document at {next_stage_path} (if that path is empty, this was the final
stage — judge readiness for merge review instead).

Decide whether the stage's changes faithfully and completely implement its plan
document and leave the codebase ready for the next stage. When you have decided,
call the `done` tool with phase="validate", status="completed", and
outputs.validation = {"passed": true|false, "findings": "...", "notes_for_next_stage": "..."}.
`findings` is a short markdown report: a one-line verdict, then `-` bullets of
what was verified and any divergences. `notes_for_next_stage` is markdown the
next stage's builder should know (surprises, renamed symbols, follow-ups) — use
"" if there is nothing. Also give a one-line markdown summary with a few `-`
bullets in the summary argument. If you cannot complete the review, call `done`
with status="blocked" or status="failed" and say, with markdown bullets, why.
```

`BUILD`, `REVISE`, `REVIEW_CHANGES`: byte-for-byte unchanged.

### 4.4 Comments rendering (`{comments}` for `revise_stage`)

Assembled **server-side** (orchestrator) from the stage's `Open` comments, in
insertion order. Exact format (one blank line between entries):

```text
1. [c-3] Under "Database schema > Tables", on the passage: "users table gets a soft-delete column"
   Comment: use a deleted_at timestamp, not a boolean

2. [c-4] (general)
   Comment: this stage feels too big, split the migration from the model changes
```

- Anchored: `N. [<comment_id>] Under "<heading_path joined with \" > \">", on the passage: "<snippet with whitespace collapsed>"`.
  When `heading_path` is empty: `N. [<comment_id>] On the passage: "<snippet>"`.
- General (anchor = None): `N. [<comment_id>] (general)`.
- Second line always `   Comment: <body>`.

The assembly function is pure: `pub fn assemble_stage_comments(comments: &[StageComment]) -> String`
in `templates.rs` (unit-tested there).

---

## 5. Persistence (`bridge/src/store.rs`)

`PersistedTask` additions — **all `#[serde(default)]`**, appended after
`last_error` and before `created_at`:

```rust
/// Multi-stage: manifest + per-stage sub-state + validation reports. Empty for
/// legacy single-plan tasks and Quick tasks — empty means "legacy path".
#[serde(default)]
pub stages: Vec<crate::task::Stage>,
/// The stage whose build/fix/validate session is (or was last) in flight.
#[serde(default)]
pub current_stage_id: Option<String>,
/// The stage a plan-revision session is running for (routes Interrupted(Plan)).
#[serde(default)]
pub revising_stage_id: Option<String>,
/// "Run all": auto-dispatch the next approved stage when validation passes.
#[serde(default)]
pub auto_advance: bool,
/// Persisted per-stage plan comments (flat; each carries its stage_id).
#[serde(default)]
pub comments: Vec<crate::task::StageComment>,
```

Serialized `StageState` examples (serde `rename_all = "snake_case"`, externally
tagged): `"planned"`, `"building"`, `{"validated":{"passed":true}}`.

**Legacy-load rule (pinned):** a record with `stages: []` (i.e., any pre-feature
file) loads and runs on the legacy path — `plan_path`, `task.plan`,
`task.approve_plan`, `task.send_notes` all behave exactly as today. Justification
in §0.4. No version field, no migration.

`ActiveTask::reattach` grows matching parameters (stages, current_stage_id,
revising_stage_id, auto_advance, comments) — see §6.1. `persist_task` in app.rs
copies them into the record.

---

## 6. Orchestrator (`bridge/src/orchestrator.rs`)

### 6.1 `ActiveTask` additions

```rust
pub struct ActiveTask {
    // ... existing fields unchanged ...
    pub stages: Vec<Stage>,                    // [] = legacy
    pub current_stage_id: Option<String>,
    pub revising_stage_id: Option<String>,
    pub auto_advance: bool,
    pub comments: Vec<StageComment>,
}

impl ActiveTask {
    pub fn is_multi_stage(&self) -> bool { !self.stages.is_empty() }
    /// The stage with `id`, or Err("unknown stage_id: <id>").
    pub fn stage(&self, stage_id: &str) -> Result<&Stage, String>
    pub fn stage_mut(&mut self, stage_id: &str) -> Result<&mut Stage, String>
    /// Index of a stage in manifest order.
    pub fn stage_index(&self, stage_id: &str) -> Result<usize, String>
    /// Open comments on one stage, insertion order.
    pub fn open_comments_for(&self, stage_id: &str) -> Vec<&StageComment>
    /// Mint the next comment id: "c-<n>", n = 1 + max numeric suffix present.
    pub fn mint_comment_id(&self) -> String
}
```

`reattach(...)` gains the five new parameters (same order as the struct). New
tasks from `dispatch()` start with `stages: vec![]`, `current_stage_id: None`,
`revising_stage_id: None`, `auto_advance: false`, `comments: vec![]`.

### 6.2 `on_done` routing (replaces the current match)

```text
fn on_done(&self, active: &mut ActiveTask, report: DoneReport) -> Result<(), OrchestratorError>
```

1. `outputs.plan_path` → update `active.plan_path` (unchanged).
2. `active.last_summary = Some(report.summary)` (unchanged).
3. Event routing:
   - `(_, Blocked)` → `TaskEvent::Blocked`; set `active.auto_advance = false`.
   - `(_, Failed)` → `TaskEvent::Failed`; set `active.auto_advance = false`.
   - `(Plan, Completed)`:
     - if `outputs.stages` is `Some(non-empty)`: initialize
       `active.stages = entries.map(Stage::from_manifest)` **merging by id** — a
       stage id that already exists keeps its `state`/`start_sha`/`validation`
       and takes the new `title`/`path`/`summary`; new ids append as `Planned`;
       ids that existed with state beyond `Approved` but are missing from the
       echo are **kept** (log a warning) — a revision must not vaporize built
       work. (First plan: the merge is trivially "all new".)
     - apply `TaskEvent::PlanReady`.
   - `(Revise, Completed)` when `active.task.state == Planning` (a stage plan
     revision):
     - apply `TaskEvent::PlanReady`;
     - for the stage in `active.revising_stage_id`: apply
       `StageEvent::Revised` via `stage_transition` (Planned/Approved → Planned;
       if the stage is in any other state this is a routing bug — return the
       `IllegalStageTransition` wrapped in a new
       `OrchestratorError::Stage(#[from] IllegalStageTransition)` variant);
     - resolve comments: for each `outputs.comment_resolutions` entry whose
       `comment_id` matches an `Open` comment with `stage_id ==
       revising_stage_id`: set `state = Addressed`, `agent_reply =
       Some(response)`. Unknown ids: log and skip. Open comments without a
       resolution stay `Open`.
     - `active.revising_stage_id = None`.
   - `(Build | Revise, Completed)` when `active.is_multi_stage()` and the
     current stage's state is `Building` (a stage build or fix session):
     - stage: `BuildDone` (→ `Built`);
     - `self.commit_all(&active.worktree.path, &format!("{} — stage {}", active.task.goal, stage.id))`
       — actually reuse `commit_all(path, goal)` with goal string
       `"{goal} — stage {stage_id}"` so the commit message is
       `Build: <goal> — stage <stage_id>`;
     - stage: `StartValidation` (→ `Validating`);
     - spawn the validation session: render `validate` with `stage_*` vars,
       `next_stage_path` = the next manifest stage's `path` or `""`,
       `stage_start_sha` = the stage's stored `start_sha` (fallback `""`);
       `end_session` first, then `spawn` (fresh cold session, same as
       `approve_plan`);
     - **no task-level event** (state stays `Building`).
   - `(Validate, Completed)` (multi-stage; current stage must be `Validating`,
     else log + ignore):
     - stage: `ValidationDone{passed}`; store
       `stage.validation = Some(report.outputs.validation.clone().expect("mcp validated"))`;
     - `end_session`;
     - if `passed`:
       - `last_stage = stage is the final manifest entry`;
       - apply `TaskEvent::ValidationPassed{last_stage}` (→ `Review` or
         `PlanReview`);
       - if `!last_stage && active.auto_advance` and the next stage's state is
         `Approved`: immediately `self.dispatch_stage(active, next_id, None)`
         (§6.3) — the task passes through `PlanReview` and lands `Building`.
     - if `!passed`: apply `TaskEvent::ValidationFailed` (→ `PlanReview`);
       `active.auto_advance = false`.
   - `(Build | Revise, Completed)` legacy (not multi-stage): `TaskEvent::BuildReady`
     (unchanged).
   - `(Plan, Completed)` with no/empty `outputs.stages`: legacy `PlanReady`
     (unchanged).
4. `active.last_error = None` on success (unchanged).

### 6.3 New orchestrator methods

```rust
/// Approve one stage's doc: Planned → Approved. Pure bookkeeping, no session.
/// Legal from any non-terminal task state (approving future stages while an
/// earlier one builds is how "run all" gets armed).
pub fn approve_stage(&self, active: &mut ActiveTask, stage_id: &str) -> Result<(), OrchestratorError>

/// Dispatch one stage's build in a fresh cold session.
/// Guards (checked in order, each a distinct error string via OrchestratorError::Gate(String)):
///   - task transition ApprovePlan must be legal (i.e. state == PlanReview);
///   - stage state must be Approved;
///   - every earlier manifest stage must be Validated{passed:true}.
/// Effects: apply TaskEvent::ApprovePlan; stage Dispatch (→ Building);
/// current_stage_id = Some(id); if start_sha is None, set it to
/// `git rev-parse HEAD` in the worktree; model_override as in approve_plan;
/// end_session; render `build_stage` with prior_notes = previous stage's
/// validation.notes_for_next_stage (or ""); spawn.
pub fn dispatch_stage(&self, active: &mut ActiveTask, stage_id: &str, model_override: Option<ModelChoice>) -> Result<(), OrchestratorError>

/// Send a stage's open comments to a fresh plan-revision session.
/// Guards: task transition SendNotes legal (PlanReview or Interrupted(Plan));
/// stage state ∈ {Planned, Approved}; at least one Open comment on the stage.
/// Effects: apply TaskEvent::SendNotes; revising_stage_id = Some(id);
/// end_session; render `revise_stage` with {comments} =
/// templates::assemble_stage_comments(open comments); spawn.
pub fn send_stage_notes(&self, active: &mut ActiveTask, stage_id: &str) -> Result<(), OrchestratorError>

/// Send a validation-failed stage back to a fresh fix session.
/// Guards: task transition ApprovePlan legal (PlanReview); stage state ==
/// Validated{passed:false}.
/// Effects: apply TaskEvent::ApprovePlan; stage Dispatch (→ Building);
/// current_stage_id = Some(id); keep existing start_sha; end_session; render
/// `fix_stage` with {findings} = stage.validation.findings, {comments} = the
/// user's note (may be ""); spawn.
pub fn fix_stage(&self, active: &mut ActiveTask, stage_id: &str, note: &str) -> Result<(), OrchestratorError>
```

Add `OrchestratorError::Gate(String)` (a rejected stage-gate precondition; message
is surfaced verbatim over RPC) and `OrchestratorError::Stage(#[from] IllegalStageTransition)`.

### 6.4 `resume` (multi-stage arm)

After `active.task.apply(TaskEvent::Reply)?` (unchanged), when
`active.is_multi_stage()` route per the §1.3 table; a stage in `Built` is moved
to `Validating` via `StartValidation` before spawning the validate session.
Legacy behavior untouched.

### 6.5 Merge review

No change to `approve_merge` / git actions. The final stage's
`validation` report rides in `task_view.stages` (§7.3) and the SPA shows it at
the Review gate (§8).

---

## 7. RPC contract (`bridge/src/app.rs`)

### 7.1 New methods

Route in `dispatch()` immediately after `"task.plan"`:

| method | request params | success result |
|---|---|---|
| `task.stages` | `{ "task_id": "task-3" }` | `{ "task_id": "task-3", "auto_advance": false, "current_stage_id": "database-schema" \| null, "stages": [ <stage view — see 7.3, but WITH a "comments" array> ] }` |
| `task.stage_doc` | `{ "task_id": "task-3", "stage_id": "database-schema" }` | `{ "stage_id": "database-schema", "path": ".build/plan/01-database-schema.md", "contents": "# …" }` |
| `task.stage_approve` | `{ "task_id": "task-3", "stage_id": "database-schema" }` | full `task_view` |
| `task.stage_dispatch` | `{ "task_id": "task-3", "stage_id": "database-schema", "model": "…"?, "effort": "…"? }` | full `task_view` |
| `task.stage_send_notes` | `{ "task_id": "task-3", "stage_id": "database-schema" }` | full `task_view` |
| `task.stage_fix` | `{ "task_id": "task-3", "stage_id": "database-schema", "note": "also run the migration in CI"? }` | full `task_view` (`note` defaults to `""`) |
| `task.comment_add` | `{ "task_id": "task-3", "stage_id": "database-schema", "body": "use a timestamp", "anchor": { "heading_path": ["Database schema", "Tables"], "snippet": "users table gets a soft-delete column" } \| null }` | `{ "comment": { "id": "c-3", "stage_id": "database-schema", "anchor": { … } \| null, "body": "…", "state": "open", "agent_reply": null } }` |
| `task.comment_delete` | `{ "task_id": "task-3", "comment_id": "c-3" }` | `{ "ok": true }` |
| `task.set_auto_advance` | `{ "task_id": "task-3", "enabled": true }` | full `task_view` |

Handler rules:

- All methods: unknown `task_id` → `"unknown task_id"`; unknown `stage_id` →
  `"unknown stage_id: <id>"`. All mutating methods go through the existing
  `take` → orchestrator → `finish_mutation` pattern so every change persists.
- `task.stages` / `task.stage_doc` are read-only (like `task.plan`), no persist.
  Both error `"not a multi-stage task"` on a legacy task. `task.stage_doc` reads
  `worktree.path.join(stage.path)`; unreadable → `"stage doc not available: <io error>"`.
- `task.comment_add`: legal while the task is non-terminal and the stage state is
  `Planned` or `Approved` (comments are plan review); otherwise error
  `"comments are only accepted on planned/approved stages (stage is <state>)"`.
  `body` required non-empty. `anchor` optional; when present, `heading_path`
  must be an array of strings and `snippet` a string (snippet capped server-side
  at 400 chars). The bridge mints the id (`mint_comment_id`).
- `task.comment_delete`: only `state == "open"` comments; else
  `"only open comments can be deleted"`. Idempotency not required — unknown id is
  `"unknown comment_id: <id>"`.
- `task.stage_send_notes`: no `comments` param — the persisted open comments ARE
  the payload. No open comments → `"no open comments on stage <id>"`.
- `task.set_auto_advance`: legal in any non-terminal state. If `enabled == true`
  and the task is currently at `PlanReview` with a dispatchable next stage
  (stage `Approved`, all prior stages `Validated{passed:true}`), the handler
  immediately dispatches it (same code path as `task.stage_dispatch`) — this is
  the run-all *trigger*. QA agent simulation applies here exactly as in
  `task_approve_plan`.

### 7.2 Changed methods (multi-stage guards)

| method | multi-stage behavior |
|---|---|
| `task.plan` | error: `"multi-stage task: use task.stages / task.stage_doc"` |
| `task.approve_plan` | error: `"multi-stage task: approve and dispatch stages individually (task.stage_approve, task.stage_dispatch)"` |
| `task.send_notes` | error: `"multi-stage task: use task.comment_add + task.stage_send_notes"` |
| everything else | unchanged (diff/merge/git_action/abandon/delete/resume operate on the worktree/lifecycle and are stage-agnostic; `task.resume` routes per §6.4) |

Legacy tasks: all methods behave exactly as today.

### 7.3 `task_view` additions

Appended to the existing object (never reordered):

```json
{
  "…existing fields…": "unchanged",
  "auto_advance": false,
  "current_stage_id": "database-schema",
  "stages": [
    {
      "id": "database-schema",
      "title": "Database schema",
      "summary": "Create the tables and the migration.",
      "path": ".build/plan/01-database-schema.md",
      "state": "validated_passed",
      "open_comments": 0,
      "validation": {
        "passed": true,
        "findings": "…markdown…",
        "notes_for_next_stage": "…markdown…"
      }
    },
    {
      "id": "api-endpoints",
      "title": "API endpoints",
      "summary": "CRUD routes over the new tables.",
      "path": ".build/plan/02-api-endpoints.md",
      "state": "approved",
      "open_comments": 2,
      "validation": null
    }
  ]
}
```

- `stages` is `[]` and `current_stage_id` is `null` for legacy/Quick tasks.
- Wire stage-state strings via a new `pub fn stage_state_str(state: &StageState) -> String`
  next to `state_str`: `"planned"`, `"approved"`, `"building"`, `"built"`,
  `"validating"`, `"validated_passed"`, `"validated_failed"`.
- `open_comments` = count of the task's comments with this `stage_id` and
  `state == Open`.
- `task.stages` returns the same stage objects **plus** a `"comments"` array per
  stage: every comment (open and addressed) for that stage, each as
  `{ "id", "stage_id", "anchor": {"heading_path": [...], "snippet": "…"} | null,
  "body", "state": "open"|"addressed", "agent_reply": "…"|null }`, insertion order.

### 7.4 QA scripted agent (`qa_agent: true`)

- `simulate_plan` (Standard tasks) now writes a deterministic **2-stage** plan:
  - `.build/plan/01-first-half.md` → `"# Stage: First half\n\n1. Implement the first half of: {goal}\n"`
  - `.build/plan/02-second-half.md` → `"# Stage: Second half\n\n1. Implement the second half of: {goal}\n"`
  - `.build/plan/stages.json` → the matching manifest (ids `first-half`,
    `second-half`; summaries `"First half."` / `"Second half."`)
  - `done(plan, completed, plan_path=".build/plan/stages.json", stages=<echo>)`.
- New `simulate_stage_build(project_id, active)`: writes
  `result-<stage_id>.txt` = `"Implemented stage <stage_id>: <goal>\n"` then
  `done(build, completed)` — which per §6.2 flips the stage to `Validating` and
  spawns the (warm no-op) validation session; the simulator then immediately
  calls `done(validate, completed, validation={passed:true, findings:"QA validation: pass.", notes_for_next_stage:"QA notes for the next stage."})`
  via `on_done`. So one QA stage dispatch lands the task back at `PlanReview`
  (or `Review` after the final stage).
- `simulate_plan` for `task.stage_send_notes`'s QA path: rewrite the stage doc
  (append `"\n(revised)\n"`), then `done(revise, completed, comment_resolutions=<one entry per open comment, response="QA: addressed.">)`.
- Quick tasks keep the existing `simulate_build` (legacy path).
- QA simulation hooks: `task_dispatch` (unchanged call site), `task.stage_dispatch`,
  `task.stage_fix`, `task.stage_send_notes`, `task.set_auto_advance` (when it
  triggers a dispatch), `task.resume`.

### 7.5 Notifications / board

No changes: validation outcomes land in `PlanReview`/`Review`, which already
notify and bucket as needs-attention. `state_str`, `STATE_LABEL`, `chipClass`
untouched.

---

## 8. SPA behavior (`spa/`)

### 8.1 markdown.js — heading ids

`renderMarkdown` emits `id` attributes on `h1`/`h2`/`h3`:
`<h2 id="database-schema">…</h2>`. Slug rule (new exported function in a new
module — see 8.2 — used by markdown.js):

```
slugifyHeading(text): strip `backticks` and ** markers, lowercase, replace every
run of non-[a-z0-9] with "-", trim leading/trailing "-". Duplicates within one
renderMarkdown call get "-2", "-3", … suffixes in document order.
```

The id is computed from the **raw heading text** (before inline HTML rendering).
Existing markdown.test.js assertions keep passing (output otherwise unchanged).

### 8.2 New module `spa/src/core/anchors.js`

```js
export function slugifyHeading(text)            // pure, unit-tested
export function buildHeadingPath(precedingHeadings)
// precedingHeadings: [{ level: 1|2|3, text: "raw heading text" }] in document
// order, containing every heading at or before the anchor point. Returns the
// enclosing chain, outermost first: walk backwards taking the nearest heading,
// then the nearest earlier heading with a strictly smaller level, until level 1
// or list start. Pure, unit-tested.
```

View-side collection (in the stages view, not unit-tested): query
`#stagedoc h1, #stagedoc h2, #stagedoc h3`, keep those positioned before the
selection's anchor node (`compareDocumentPosition`), map to `{level, text}`,
pass to `buildHeadingPath`.

### 8.3 New module `spa/src/views/stages.js`

Exports:

```js
export const STAGE_LABEL = {
  planned: "PLANNED", approved: "APPROVED", building: "BUILDING",
  built: "BUILT", validating: "VALIDATING",
  validated_passed: "VALIDATED", validated_failed: "VALIDATION FAILED",
};
export function stageChipClass(state)
// planned → "", approved → "attn", building/built/validating → "work",
// validated_passed → "done", validated_failed → "warn"
export function renderStagesTab(ctx)
// ctx: { body, task, stagesData, selectedStageId, onSelectStage, callRpc, repaint }
```

### 8.4 Plan tab rewiring (`spa/src/views/task.js`)

`paint()`, plan branch: if `t.stages && t.stages.length` → multi-stage flow;
else the existing legacy flow (renderPlanTab) untouched.

Multi-stage flow:

- Each poll (plan tab active) additionally calls `task.stages` for comment
  bodies. Rebuild key: `t.state + " " + JSON.stringify(stagesData)`; skip
  rebuild when unchanged or when the user is mid-comment (`hasCommentPop()` or a
  focused/non-empty note input) — same freeze pattern as the diff tab.
- **Stage list** (default view): header row with the task-level controls, then
  one row per stage in manifest order: `NN`, title, stage chip
  (`STAGE_LABEL`/`stageChipClass`), open-comment count badge (`N 💬` when > 0),
  summary line. Click → stage doc view.
- **Header controls:**
  - Run-all toggle: a checkbox labeled `Run all (auto-advance)` reflecting
    `t.auto_advance`, calling `task.set_auto_advance { enabled }` on change.
  - When every stage is `planned`: an `Approve all` button looping
    `task.stage_approve` over planned stages.
- **Stage doc view** (per stage):
  - Back link to the stage list.
  - **Validation banner — the previous stage's report** (decision: the report is
    surfaced on stage N+1): if the previous manifest stage has
    `validation != null`, render a banner at the top:
    passed → class `stage-validation pass`, heading
    `Validation of "<prev title>" passed`, body = `notes_for_next_stage`
    rendered as markdown (omit the banner body if empty, keep the heading);
    failed → class `stage-validation fail`, heading
    `Validation of "<prev title>" failed`, body = `findings` markdown.
  - **Own-stage failure banner:** if THIS stage is `validated_failed`, render its
    own `findings` in a `stage-validation fail` banner above the doc, plus a
    note textarea and a `Send back to fix` button → `task.stage_fix
    { task_id, stage_id, note }`.
  - The doc: `task.stage_doc` contents through `renderMarkdown` into
    `<div class="plan" id="stagedoc">`.
  - **Persisted comments:** listed under the doc (open first): heading-path
    breadcrumb (` > ` joined; `(general)` when null) + snippet + body; addressed
    comments show the `agent_reply` and a muted style; open comments get an `×`
    calling `task.comment_delete`. Clicking a comment's breadcrumb scrolls to
    `#<slugifyHeading(last heading in path)>`.
  - **Adding comments** (stage state `planned`/`approved` only): reuse
    `watchSelection` + `showCommentPop` on `#stagedoc`; on save, compute
    `heading_path` (8.2), `snippet = selection text trimmed, ≤400 chars`, call
    `task.comment_add`, repaint. A general-comment textarea + `Add comment`
    button posts `anchor: null`.
  - **Per-stage actions** (bottom action bar, by stage state):
    - `planned` → `Approve stage` → `task.stage_approve`; plus, if open
      comments > 0, `Send N comments` → `task.stage_send_notes`.
    - `approved` → `Start stage` (disabled with hint `waiting on validation of
      "<prev>"` unless all prior stages are `validated_passed` and task state is
      `plan_review`) → `task.stage_dispatch` (with the existing model/effort
      selects, reused from the legacy approve bar); plus `Send N comments`
      when open comments exist.
    - `building`/`built`/`validating` → no actions; hint `agent working on this
      stage…` / `validation running…`.
    - `validated_failed` → the fix banner above owns the action.
    - `validated_passed` → no actions; hint `stage complete`.
- **Review gate:** when `t.state === "review"` and the task is multi-stage, the
  plan tab's stage list shows the final stage's validation banner at the top
  (`Validation of "<final title>" passed` + findings) — the merge decision
  context. (The Diff tab and git split-button are unchanged.)
- Modules **reused as-is**: `selectWatch.js`, `commentPop.js`, `markdown.js`
  (extended), `modelPicker.js`, `taskActions.js`, `shared.js`, poll/banner
  machinery in task.js. `notes.js` stays for the legacy plan flow and the diff
  flow only — multi-stage comment assembly is server-side, so `assemblePlanNotes`
  is NOT used by the stage flow.

### 8.5 CSS

Add to `spa/src/styles.css`: `.stagerow`, `.stagechip`, `.stage-validation`,
`.stage-validation.pass`, `.stage-validation.fail`, `.commentcard`,
`.commentcard.addressed` — follow the existing chip/banner palette
(`--green`/`--red`/existing `chip` classes). Exact styling is the implementer's
call; class names above are pinned so tests can assert them.

---

## 9. Per-layer task list (six sequential implementation agents)

House rules for every layer: TDD (failing test first), then
`cd bridge && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt`
(layers 1–5) or `cd spa && npm test` (layer 6), then `semgrep --config auto` on
changed files + `gitleaks protect --staged`, then commit on
`multi-stage-planning` with the required trailers.

### Layer 1 — domain core (`bridge/src/task.rs`)

- **Files:** `bridge/src/task.rs` only.
- **Tests first (in-file `mod tests`):**
  - `stage_transition` full table + illegal cases (incl. `Validated{passed:true}`
    rejects everything, `Building` rejects `Approve`/`Dispatch`).
  - `Revised` resets `Approved → Planned` and keeps `Planned → Planned`.
  - `transition`: the six new `ValidationPassed`/`ValidationFailed` arms, plus
    rejections (`ValidationPassed` illegal from `PlanReview`, `Review`,
    `Planning`, `Created`, terminals).
  - Serde round-trips: `StageState` (incl. `{"validated":{"passed":false}}`),
    `Stage`, `StageComment` with and without `anchor`/`agent_reply`,
    `ValidationReport`, `StageManifestEntry` without `summary` (defaults `""`).
  - `Stage::from_manifest` starts `Planned` with empty extras.
- **Add:** everything in §1.1–§1.2 exactly.
- **Must NOT touch:** existing `TaskState` variants, existing `transition` arms,
  `needs_attention`/`is_working`/`is_terminal`, any other file.

### Layer 2 — agent interface (`bridge/src/mcp.rs`, `bridge/src/templates.rs`)

- **Files:** `bridge/src/mcp.rs`, `bridge/src/templates.rs`.
- **Tests first:**
  - mcp: `validate/completed` without `validation` → tool error
    (`MissingValidationReport` text); with it → report emitted carrying the
    parsed `ValidationReport`. `plan/completed` with a bad manifest echo
    (duplicate id; bad id chars; path outside `.build/plan/`; empty title) →
    `InvalidStages` tool error naming the offense. Good manifest echo →
    report carries the entries. `revise/completed` with `comment_resolutions`
    round-trips. `tools/list` schema now enumerates `validate` and the new
    output properties. All existing tests keep passing unmodified.
  - templates: new templates contain their pinned `phase="…"` strings,
    placeholders, "markdown"/"bullet" wording (extend the existing loop test to
    the four new fields); `render` substitutes every new `{var}`;
    `assemble_stage_comments` produces the exact §4.4 format for anchored /
    empty-heading-path / general comments.
- **Add:** §3 types/rules/schema; §4 Vars/struct/templates/constants/assembler.
- **Must NOT touch:** `BUILD`/`REVISE`/`REVIEW_CHANGES` texts, `DEFAULT_PLAN_PATH`,
  the JSON-RPC framing in mcp.rs, any other file.

### Layer 3 — persistence (`bridge/src/store.rs`)

- **Files:** `bridge/src/store.rs`.
- **Tests first:**
  - A pre-feature record (serialize today's record, then `remove` the five new
    keys — same fixture style as `task_files_from_before_model_choice_still_load`)
    loads with `stages: []`, `current_stage_id: None`, `revising_stage_id: None`,
    `auto_advance: false`, `comments: []`.
  - Full round-trip of a record with two stages (one
    `Validated{passed:false}` with a report + `start_sha`), two comments (one
    anchored/open, one general/addressed with `agent_reply`), `auto_advance:
    true`, `current_stage_id`/`revising_stage_id` set.
- **Add:** the five §5 fields on `PersistedTask` (update the in-file `record()`
  fixture).
- **Must NOT touch:** save/load/delete logic, atomicity, error types, any other
  file.

### Layer 4 — orchestrator (`bridge/src/orchestrator.rs`)

- **Files:** `bridge/src/orchestrator.rs`.
- **Tests first (warm no-op harness; the test plays every agent, as today):**
  - Multi-stage happy path: dispatch → `done(plan, stages echo)` → `PlanReview`
    with two `Planned` stages → `approve_stage` + `dispatch_stage(s1)` →
    `Building`, stage `Building`, `start_sha` set → `done(build)` → stage
    `Validating`, task still `Building`, work committed →
    `done(validate, passed:true)` → task `PlanReview`, stage
    `validated_passed` with report → `dispatch_stage(s2)` uses `prior_notes` →
    `done(build)`, `done(validate, passed:true)` → task `Review` (last stage).
  - Gate rejections: `dispatch_stage` on an unapproved stage; on stage 2 while
    stage 1 isn't `Validated{passed:true}`; from a non-`PlanReview` state.
  - Validation failure: `done(validate, passed:false)` → task `PlanReview`,
    stage `validated_failed`, `auto_advance` flipped false; `fix_stage` →
    `Building` with the findings in the prompt (use a recording OneShot agent to
    capture prompts); `start_sha` unchanged; second `done(build)` +
    `done(validate, passed:true)` proceeds.
  - Auto-advance: `auto_advance = true`, stage 2 pre-approved →
    `done(validate s1, passed:true)` lands the task `Building` on stage 2
    without any human call; with stage 2 NOT approved it lands `PlanReview`.
  - Comments/revision: `comment_add`-shaped setup (push comments on the
    ActiveTask directly), `send_stage_notes` → `Planning`,
    `revising_stage_id` set, prompt contains the §4.4 rendering;
    `done(revise, comment_resolutions)` → `PlanReview`, comment `Addressed`
    with reply, unresolved comment stays `Open`, stage back to `Planned`
    (approval reset), `revising_stage_id` cleared.
  - `blocked` during validation → `Blocked(Build)`, `auto_advance` flipped
    false, stage stays `Validating`.
  - Resume routing per §1.3 (reattach with each stage sub-state and assert the
    respawned template via the recording agent).
  - Legacy: every existing test passes **unmodified** (plan without a stages
    echo, quick tasks, merge honesty, abandon).
- **Add:** §6 in full.
- **Must NOT touch:** `task.rs`/`mcp.rs`/`templates.rs`/`store.rs` beyond using
  them, merge/commit/push logic, worktree management, `app.rs`.

### Layer 5 — RPC (`bridge/src/app.rs`)

- **Files:** `bridge/src/app.rs`.
- **Tests first (QA-agent driven, `handle(req(...))` style):**
  - Full multi-stage QA lifecycle over RPC: `task.dispatch` (standard) →
    `task.stages` shows 2 planned stages → `task.comment_add` (anchored +
    general) → view `open_comments` counts → `task.stage_send_notes` →
    comments addressed with replies, stage back to `planned` →
    `task.stage_approve` ×2 → `task.stage_dispatch(first-half)` → QA simulation
    lands `plan_review` with stage 1 `validated_passed` and a report →
    stage 2 doc surfaces… (assert `task.stages` payload shapes exactly per
    §7.1/§7.3) → `task.stage_dispatch(second-half)` → `review` →
    `task.approve_merge` merges both stages' files.
  - `task.set_auto_advance` triggers the pending dispatch; the whole run-all QA
    flow reaches `review` from one `set_auto_advance(true)` after approving all.
  - Multi-stage guards on `task.plan` / `task.approve_plan` / `task.send_notes`
    (exact error strings, §7.2).
  - `task.comment_delete` open-only rule; unknown ids.
  - Persistence round-trip: drive to mid-flight (stage 1 validated, comments
    present, auto_advance on), rebuild `AppState` `with_task_store`, assert
    `task.stages`/`task.get` match and `Interrupted` routing resumes correctly
    via `task.resume`.
  - Legacy record loads (no stage keys) and behaves as before, incl. `task.plan`.
  - Existing standard-task tests: update them to the stage flow where they used
    `task.approve_plan` (the QA agent is now multi-stage for standard tasks);
    quick-task tests unchanged.
- **Add:** §7 methods/guards/`task_view` fields/`stage_state_str`/QA simulators;
  `persist_task` + `recover_task` + `reattach` call sites carry the new fields.
- **Must NOT touch:** relay/terminal/stream/pairing/notify code, `main.rs`
  (no CLI changes needed), any spa file.

### Layer 6 — SPA (`spa/`)

- **Files:** `spa/src/core/markdown.js`, new `spa/src/core/anchors.js`, new
  `spa/src/views/stages.js`, `spa/src/views/task.js`, `spa/src/styles.css`;
  tests in `spa/test/markdown.test.js`, new `spa/test/anchors.test.js`, new
  `spa/test/stages.test.js`.
- **Tests first (vitest, pure functions — follow the existing test style):**
  - markdown: headings get slug ids; duplicate headings get `-2` suffixes;
    non-heading output unchanged from the existing assertions.
  - anchors: `slugifyHeading` (backticks/bold stripped, punctuation runs → `-`);
    `buildHeadingPath` chains (h2 under h1; h3 under h2 under h1; leading text
    before any heading → `[]`; sibling h2s pick the nearest).
  - stages: `STAGE_LABEL` completeness for all seven wire states;
    `stageChipClass` mapping.
- **Then wire the views** per §8.4 (renderStagesTab + task.js branching + CSS).
- **Must NOT touch:** the diff tab flow, `notes.js`, `selectWatch.js`,
  `commentPop.js`, terminal/board/gate/settings views, service worker, api.js.

---

## 10. Cross-layer invariants (checked by the final layer's end-to-end QA test)

1. A legacy persisted task file (no new keys) boots, lists, plans, approves,
   merges — byte-identical behavior to today.
2. Quick tasks never gain stages.
3. `stages` non-empty ⇔ multi-stage code paths; there is no third mode.
4. Every stage mutation persists via `finish_mutation` before the RPC returns.
5. The task machine and stage machine stay pure — no IO in `task.rs`.
6. Stage N+1 can never reach `Building` unless stage N is `Validated{passed:true}`
   (enforced in `dispatch_stage`, asserted in tests at both layers 4 and 5).
7. `auto_advance` is false after any validation failure, `blocked`, or `failed`.
