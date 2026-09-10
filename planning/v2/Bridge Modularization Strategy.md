# Bridge Modularization Strategy

**Status:** Stages 1–9 implemented; lifecycle reversal and follow-on leaf splits remain
**Scope:** Rust bridge structure and dependency direction; no wire or behavior change

## Why this work is needed

`bridge/src/app.rs` is 61,274 physical lines, 44.4% of all Rust under
`bridge/src`. Its production portion is already a large subsystem: the first
test module begins at `app.rs:23774`, leaving 23,773 lines before tests. The
test module contributes about 37,501 lines, with 739 inline test cases plus 7
included merge-regression tests.

The problem is concentration of responsibilities, not only file length:

- `AppState` has 61 fields spanning project and entity registries, config,
  persistence, captures, attention, diff caches, lifecycle reservations,
  terminal sessions, delivery, operation receipts, RTC, pushes, and telemetry
  (`bridge/src/app.rs:1877-2134`).
- One string dispatcher has 119 literal RPC arms and mixes conversations,
  filesystem access, git, settings, projects, board, captures, issues, runs,
  branches, agents, streams, and terminals (`bridge/src/app.rs:7026-7174`).
- Production helpers, off-lock job types, lifecycle settlements, session
  delivery, and recovery continue after that dispatcher; tests then exercise
  all of those concerns through one private namespace.
- The complexity ratchet records 19 of its 28 existing suppressions in
  `app.rs` (`bridge/tests/complexity_ratchet.rs:12-18`). Splitting files alone
  will improve navigation, but the end state also needs narrower components
  so those functions can be reduced.

This concentration now bends dependency direction. `lifecycle.rs` imports
`AppState`, `OffLockGate`, and many app-owned epilogue/caller types
(`bridge/src/lifecycle.rs:24-61`, `bridge/src/lifecycle.rs:372-448`).
`screen.rs` imports an app base64 helper (`bridge/src/screen.rs:20`), and
`gitgui.rs` reaches back into app for its canonical containment fence
(`bridge/src/gitgui.rs:1382-1412`). A leaf module that depends on the top-level
coordinator is hard to reuse and hard to test in isolation.

## Existing seams to preserve

Several boundaries already have the right shape and should guide the split.

- `orchestrator.rs` explicitly owns plan/run transitions and takes active
  entities from the caller instead of reaching into app maps
  (`bridge/src/orchestrator.rs:1-40`). Its cloneable `Orchestrator` is designed
  to carry slow checkout work beyond the app lock
  (`bridge/src/orchestrator.rs:1110-1163`).
- `lifecycle.rs` already defines decide, run, and apply phases. Its
  `WorktreeMutation::perform` owns all slow inputs and cannot receive
  `&mut AppState` (`bridge/src/lifecycle.rs:1-18`,
  `bridge/src/lifecycle.rs:68-95`). Keep this protocol while moving settlement
  ownership out of app.
- `screen.rs` owns a separate screen lock, so terminal parsing and flow control
  do not require direct access to `AppState` (`bridge/src/screen.rs:1-12`).
- `Store` is cloneable around one mutex-protected SQLite connection and exposes
  an explicit transaction helper (`bridge/src/store.rs:685-708`,
  `bridge/src/store.rs:866-891`). That transaction boundary is more important
  than the eventual file boundary.
- `WorktreeManager` already routes checkout creation through isolation
  backends (`bridge/src/worktree.rs:200-260`), and `harness_for` remains the one
  provider dispatch.
- Agents are durable identities and a branch may carry any number of them
  (`bridge/src/agent.rs:1-12`). Session registries and spawn reservations must
  therefore remain keyed by canonical root plus agent id, as `TabKey` already
  is (`bridge/src/app.rs:347-378`, `bridge/src/app.rs:2027-2038`).

## Target layout

Keep `bridge/src/app.rs` as the public facade during the migration. Rust allows
it to declare children in `bridge/src/app/`, so there is no need for an early
`app.rs` to `app/mod.rs` rename. Preserve the current `build_bridge::app::*`
paths with facade re-exports until callers have intentionally moved.

```text
bridge/src/app.rs                  AppState facade, construction, shared lock
bridge/src/app/
  rpc.rs                          frame envelope, dispatch table, response tail
  fs.rs                           scoped filesystem request adapter
  git.rs                          scoped git-GUI request adapter and off-lock jobs
  config.rs                       settings and persisted project configuration
  projects.rs                     project add/create/clone/remote/isolation
  conversations.rs                thread read/post/attach and operation receipts
  captures.rs                     capture CRUD and routing coordination
  issues.rs                       issue/plan RPC application service
  runs.rs                         run/stage/review/finish RPC application service
  worktrees.rs                    branch/worktree/adoption application service
  board.rs                        board views, attention, archive, diff-cache views
  mcp.rs                          local MCP listener, token checks, action routing
  rtc.rs                          application-facing signaling/session adapter
  runtime/
    mod.rs                        runtime facade
    sessions.rs                   TabKey, Tab, terminal/agent registry and pumps
    spawning.rs                   per-agent claims and session publication
    delivery.rs                   queued/in-flight turns and delivery outcomes
    deferred.rs                   decide/run/apply jobs and result application
    recovery.rs                   boot recovery and vanished-run reconciliation
  tests/
    mod.rs                        shared fixtures only
    rpc.rs, conversations.rs, projects.rs, issues.rs, runs.rs
    worktrees.rs, runtime.rs, recovery.rs
```

These are ownership destinations, not a request to create every file at once.
At first, child modules may contain `impl AppState` blocks and use the same
state fields. That mechanical split makes later ownership changes reviewable.
The end state replaces groups of fields with narrow components such as
`SessionRegistry`, `DeliveryQueue`, `ProjectRegistry`, `ConversationService`,
and `BoardIndex`. Their methods should accept their own inputs and state, never
the entire `&mut AppState`.

The rest of the crate should follow once app is stable:

```text
thread/    item and wire types | metadata/search | paging | catch-up rendering
store/     connection/schema   | entities        | conversations/operations
           canonical documents | legacy import
worktree/  manager/backends    | discovery       | identity/comparison
gitgui/    status/log/show     | patches         | mutations/branches
harness/   keep current provider/session boundary; split large ADK protocol
           state, reader/translation, activity reporting, and tests
orchestrator/ keep its facade; split plan, run, launch/report, and workspace
              implementation behind the current caller-owned-state contract
```

This follows visible responsibility boundaries. `thread.rs` currently holds
wire item types, metadata extraction, page cutting, queries, catch-up rendering,
and the large `Thread` implementation (`bridge/src/thread.rs:93-1753`,
`bridge/src/thread.rs:1817-3421`). `store.rs` combines schema/migrations,
operation receipts, conversation paging, all entity repositories, legacy JSON
import, and canonical plan documents (`bridge/src/store.rs:386-683`,
`bridge/src/store.rs:1213-2343`). `worktree.rs` combines managed checkout
mutation with external checkout discovery and summaries
(`bridge/src/worktree.rs:214-998`, `bridge/src/worktree.rs:1187-1528`).

## Invariants that govern every extraction

1. Keep one `Arc<Mutex<AppState>>` coordinator during this project. The crate
   denies holding this lock across `await`, and the app's off-lock protocols
   exist because process, git, filesystem, and spawn work can block. A module
   move must not pull blocking run work back under the mutex; the short apply
   phase intentionally reacquires it.
2. Preserve prepare/run/apply ownership. Decide under the lock, run with owned
   inputs and no state reference, apply under the lock. Retain lifecycle row,
   checkout-finish, diff-refresh, and spawn claims through publication or
   supersession; they are concurrency correctness, not temporary plumbing.
3. Preserve application transactions. A reviewer post, conversation-owner
   record, operation receipt, and delivery intent are accepted as one durable
   unit (`bridge/src/app.rs:3202-3252`, `bridge/src/app.rs:13640-13729`;
   `bridge/src/store.rs:1734-1750`, `bridge/src/store.rs:1825-1840`). If a save
   refuses, the in-memory thread is restored. Only turns introduced by the
   refused request may be considered for dropping, and any already durable turn
   marked `survives_refusal` must remain (`bridge/src/app.rs:7001-7023`).
   Deliver only after durable acceptance succeeds. Preserve mutation-tail
   ordering, including restoring entities before attention and notification
   bookkeeping (`bridge/src/app.rs:3346`, `bridge/src/app.rs:3386`).
4. Keep operation validation scoped to the resolved owner, agent, and
   conversation. Preserve immutable operation payloads and the exact
   start/end acknowledgement bounds; do not turn a refactor into a broader
   authorization lookup (`bridge/src/app.rs:5597`).
5. Preserve public wire methods, parameter aliases, response shapes, event
   ordering, error text where tested, and `build_bridge::app` entry points.
6. Treat comments as evidence only when they match current types. In
   particular, stale “one agent per worktree” prose in `app.rs` must not
   override the multi-agent contract in `agent.rs` or the `TabKey` shape.

## Staged implementation

### PR 1: Make source guards relocation-safe

**Implemented and reviewed.** Validation is recorded below; the existing
concurrency load benchmark also fails on the unchanged baseline in this environment.

Before moving production code, update the harness dispatch guard. Its baseline
module list reads only Rust files directly under `src` and asserts that it saw
`src/app.rs` (`bridge/src/harness/mod.rs:1182-1219` at `0d52029a`). Extend the
scan to shipped
children under `src/app/`, while excluding `#[cfg(test)]` modules and the
harness implementation subtree. Add a fixture proving a forbidden provider
dispatch in a nested app module is caught.

Record baseline line, test, route-arm, and complexity counts in the PR. Counts
are diagnostics; the enforced rule remains that suppressions only decrease.

Stage 1 baseline (`0d52029a`, before code moves):

| Diagnostic | Count |
| --- | ---: |
| Physical lines in `app.rs` | 61,274 |
| Lines before the main test section | 23,773 |
| Lines in the main test section | 37,501 |
| Inline app test cases / included merge-regression cases | 739 / 7 |
| Literal methods in `AppState::route` / `dispatch_frame` | 119 / 13 |
| Distinct methods across both dispatchers | 132 |
| Complexity suppressions in app / ratcheted crate total | 19 / 28 |

The method sets have no overlap. These measurements describe the starting
point; the existing complexity test remains the enforcement mechanism.

The stage 1 guard implementation (`bridge/src/harness/mod.rs:991`) walks nested
source directories and prunes
both file-backed and directory-backed `#[cfg(test)]` modules. Exemptions remain
anchored to the root provider table and harness implementation; an application
child named `models.rs` is still checked. Temporary source-tree regressions
exercise a nested forbidden dispatch under an `app/mod.rs` facade and the
test-only exclusions. This is a source-layout guard for conventional Rust
modules, not a general resolver for `#[path]`, macros, or arbitrary cfg logic.

Validation on 2026-09-09:

- The nested-dispatch regression fails with the old direct-root scanner and
  passes with the recursive scanner. All 21 focused harness tests pass.
- `cargo fmt --all -- --check` and
  `cargo clippy --all-targets -- -D warnings` pass.
- `cargo test --all` passes 2,059 library tests (6 ignored), 9 binary tests,
  2 CLI tests, and both complexity-ratchet tests, then stops at
  `concurrency_load`. Running the remaining integration targets explicitly
  passes 62 tests (1 ignored); `cargo test --doc` passes with no doctests.
- The load test fails its existing terminal-output minimum at
  `bridge/tests/concurrency_load.rs:152`: 77,820 bytes versus a required
  count greater than 100,000, both in the suite and on an isolated retry.
  Restoring the original harness source from `0d52029a` reproduces the same
  assertion failure (75,226 bytes). The final guard source was restored
  byte-for-byte afterward. No load-test thresholds or runtime behavior changed.
- Live Rust/Python interop is not exercised because
  `BUILD_SECURE_TRANSPORT_PY` is unset; its integration target returns early.
- Semgrep reports no findings on the changed Rust file; Gitleaks reports
  no secrets in the harness tree. Independent source review finds no blockers.

### PR 2: Extract the app test tree

The `#[cfg(test)]` body formerly beginning at `bridge/src/app.rs:23774` now
lives in `app/tests/`. The facade is 23,775 lines, down from 61,274; its
23,773-line production prefix is byte-for-byte unchanged. The seven former
`app_merge_regressions.rs` tests are an ordinary child module rather than an
`include!` expansion.

Capability directories cover configuration, conversations, workflow, runtime,
git, board, protocol, and routing. Smaller root modules cover filesystem,
shell, harness models, push, RTC, and merge regressions. Each capability has
separate scenario files to avoid replacing the original monolith with another
oversized test file. The tree contains 59 Rust files; the largest scenario file
is 1,583 lines.

`tests/mod.rs` keeps the common imports, request/screen helpers, and untimed
delivery wrappers. `tests/support/` owns shared session fixtures and workflow
builders, and provides a common import surface for fixtures owned by a specific
capability. Cross-module fixture visibility is restricted to `crate::app::tests`;
production visibility remains unchanged. Tests that intentionally call the real
timed pump functions now name `crate::app::spawn_*`, preserving the distinction
from the untimed wrappers after relocation.

Preservation checks compare all 746 test cases and 222 helper items with the
pre-extraction source, including decoded literal values inside macros. Only
test-module visibility and the necessary production-pump paths are normalized.
The remaining syntax differences were reviewed as rustfmt-only changes
(trailing commas and a redundant closure block). Compiled test discovery also
checks the complete app test-name inventory; the test module paths change with
the capability layout, while test function names remain unchanged.

Validation on 2026-09-09:

- `cargo fmt --all -- --check` and
  `cargo clippy --all-targets -- -D warnings` pass.
- Compiled discovery retains all 746 app tests and 2,065 total library test
  cases. Independent source review preserves all 222 helper items, test
  attributes, assertions, and fixture values; it finds no extraction blocker.
- `cargo test --all --no-fail-fast` exercises every target and reports 2,131
  passed, 4 failed, and 7 ignored. The library accounts for 2,058 passed,
  1 failed, and 6 ignored; the other targets account for 73 passed, 3 failed,
  and 1 ignored. Doctests pass with no cases.
- The unchanged push test `an_entity_change_names_the_entity_that_moved`
  observes two notification pairs instead of one during the parallel suite;
  an isolated retry passes. Its setup uses a fixed 300 ms settling wait while
  the captured background delivery takes 356.9 ms, consistent with setup
  notifications spilling into the assertion window. The test and its helper
  chain are unchanged apart from test-only visibility.
- `concurrency_load` again fails its pre-existing output-floor assertion at
  `tests/concurrency_load.rs:152`, with 75,226 bytes. Stage 1 reproduced this
  assertion on the original source; no runtime code or threshold changed here.
- Two unchanged Pi extension tests exceed their four-second wall-clock
  assertions in the full run. Their integration target is independent of
  `app/tests/`; rerunning the complete target with `--test-threads=1` passes
  all 17 tests, including both previously timed-out cases.
- Semgrep reports no findings with 51 applicable rules on `app.rs` and all
  59 extracted Rust test files. The test-tree scan explicitly includes files
  normally excluded by the repository's test-directory ignore rule. Gitleaks
  reports no secrets in the staged diff.
- Live Python interop remains unexercised because
  `BUILD_SECURE_TRANSPORT_PY` is unset; configured ignored tests remain ignored.

#### Sync with main

The stage-two branch incorporates `main` at `09220956` (also the fetched
`origin/main` tip on 2026-09-09). Its only merge conflict was the old inline
app test section. The resolved `app.rs` keeps main's production prefix
byte-for-byte and retains the out-of-line test module; it is now 24,097 lines.

Upstream adds six net app tests, bringing the inventory to 752. The relocated
changes cover independent agent-mode settings, atomic settings persistence,
project-list work outside the app lock, and file modification timestamps in
diff responses. They live in the existing configuration, git, and workflow
test modules. The rest of main's Rust and frontend changes merged directly.

The source audit matches all 752 tests and 222 helpers to this main snapshot.
Its 29 strict differences are 28 inherited formatting changes and one reworded
test comment; no executable assertion or helper changes are introduced by the
resolution. The frontend index and working tree also match this snapshot.

Frontend lint and build pass. The full frontend test run was stopped without
a final suite summary after prolonged execution; an isolated retry of
`agentRailDom.test.js`'s working-status collapse test fails at line 2986,
expecting `1:25` but rendering `Working 1:36`. This test is unchanged from the
selected main revision. Later commits arriving on main during validation are
outside this fixed merge snapshot.

Merged Rust validation discovers 2,105 library test cases, including all 752
app tests. `cargo test --all --no-fail-fast` finishes with 2,174 passed,
1 failed, and 7 ignored; the library has 2,099 passed, no failures, and
6 ignored. All other targets pass except the previously reproduced
`concurrency_load` output-floor assertion, now measuring 77,820 bytes.
Formatting and `cargo clippy --all-targets -- -D warnings` pass. Semgrep reports
no findings across 244 production Rust and frontend files and all 59 extracted
test files; staged Gitleaks passes.
Live Python interop remains unexercised with its environment variable unset.

#### Follow-up sync with main

The next sync incorporates `124a4bc1`, matching both main and origin/main when
fetched. It includes opt-in diff triage, authoritative Codex subagent metadata,
and expanding folded projects when creating a branch. The app facade is now
24,138 lines, with this main snapshot's production prefix unchanged.

Five new upstream tests live in the existing settings and workflow-triage
modules. Existing tests and the shared `triaged_run` helper explicitly enable
triage where upstream does. The source audit preserves all 757 app tests and
222 helpers; its 29 formatting/comment differences are inherited unchanged
from the prior sync, with no new executable difference.

Frontend lint and build pass, and all 150 tests in the seven updated frontend
test files pass. The frontend source matches the selected main snapshot
exactly. The broader frontend suite's previously recorded timing limitation
was not re-exercised for this follow-up.

Rust discovery finds 2,115 library test cases, including the 757 app tests.
`cargo test --all --no-fail-fast` finishes with 2,184 passed, 1 failed, and
7 ignored; the library has 2,109 passed, no failures, and 6 ignored. The only
failed target is the previously reproduced `concurrency_load` output floor,
measuring 75,226 bytes. All other targets pass. Formatting and all-target Clippy
with warnings denied pass. Semgrep reports no findings across 245 production
Rust/frontend files and all 59 extracted test files; staged Gitleaks passes.
Live Python interop remains
unexercised with `BUILD_SECURE_TRANSPORT_PY` unset.

### PR 3: Remove leaf-to-app utility dependencies

**Implemented.** Base64 wire helpers now live in `encoding.rs`, while lexical
and canonical path validation share `fs_scope.rs`. `screen.rs` and `gitgui.rs`
use these owners directly. Shell resolution and login PATH capture live in
`terminal_environment.rs`; their existing public app paths remain available,
as does the public plan path validator. The independent QA file writer lives
in `app/qa.rs`; stateful QA lifecycle adapters remain for the later extraction.

All 84 focused tests, formatting, all-target check, and all-target Clippy pass.
The full suite reports 2,184 passed, one failed, and seven ignored: the sole
failure is the previously reproduced `concurrency_load` output floor at
75,226 bytes. All 2,109 exercised library tests pass. Source auditing preserves
all 5,434 functions and every test body, with only the expected path-validation
relocations requiring review. Semgrep and staged Gitleaks pass. The app facade
is now 24,035 lines. See [the progress record](Bridge%20Modularization%20Progress.md)
for the remaining integration sequence and validation evidence.

### Phases 4-6: Split the application services mechanically

**Stage 4 implemented.** Conversation addressing, reads, posting, attachments,
and input parsing live in five modules under `app/conversations/`, each under
700 lines. `app.rs` is 21,873 lines. Public attachment limits remain available
through the app facade; cross-record persistence remains with the coordinator.
All 5,434 functions and every test body are preserved; the audit's eight
reviewed signature pairs differ only by rustfmt trailing commas. Focused
conversation tests pass 51 cases with one configured ignore. Formatting,
compilation, all-target Clippy, Semgrep, and Gitleaks pass. The full suite
reports 2,184 passed, one failed, and seven ignored, with all 2,109 exercised
library tests passing. The only failure is the known `concurrency_load`
output floor at 80,414 bytes; no threshold or timing behavior changed.

Move one cohesive route family per PR into `app/conversations.rs`, then
projects/config/captures, then issues/runs/worktrees/board. The central
dispatcher delegates to family dispatchers or a typed route table; it remains
the only place that maps public method names. Use temporary child-module
`impl AppState` blocks and `pub(super)` access rather than widening items to
`pub(crate)`.

Each PR should move its matching tests and retire complexity suppressions where
the new boundary makes a function naturally smaller. Avoid mixing code moves,
renames, behavior changes, and public API changes in one diff.

### PRs 7-9: Give runtime subsystems their state

Extract the session registry and per-agent spawn claims first, delivery queues
and receipts second, then board attention and caches. Each component gets a
narrow interface and owns its invariants. `AppState` becomes composition plus
cross-component transaction ordering. Keep `bridge.stats` readable without
waiting for the app mutex, as its current field contract requires
(`bridge/src/app.rs:2123-2133`).

### PR 10: Reverse the lifecycle dependency

Move app-owned lifecycle epilogues and caller traits behind lifecycle-owned
typed results or narrow settlement interfaces. `lifecycle.rs` should return
owned facts; an app runtime adapter applies them. The lifecycle module should
no longer name `AppState`, while decide/run/apply behavior and rollback order
remain unchanged.

### Follow-on: Split the other large modules

Apply the same facade-first pattern to `thread.rs`, `store.rs`, `orchestrator.rs`,
`worktree.rs`, `gitgui.rs`, and `harness/adk.rs`, one module at a time. Split `Store` by
repository responsibility without creating separate SQLite connections or
breaking multi-record transactions. Split thread algorithms behind one item
classification contract so memory and SQLite paging cannot drift.

## Validation for every PR

Run from `bridge/`:

```text
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

These are the repository and CI gates (`CLAUDE.md:25-49`,
`.github/workflows/ci.yml:121-128`). Also run focused tests for the extracted
family while iterating, then the full suite before review. PRs touching
transport or frame routing also run the existing integration targets relevant
to that boundary; CI separately runs the Rust/Python transport interop test
(`.github/workflows/ci.yml:130-150`).

Track these ratchets in review:

- `app.rs` production and test lines decrease every extraction; no replacement
  module becomes a new oversized catch-all.
- The complete baseline method inventory remains covered exactly once: the 119
  arms in `AppState::route` plus session, RTC, and terminal methods in the
  separate frame dispatcher (`bridge/src/app.rs:21241`).
- Cognitive-complexity suppressions never increase; lower the asserted 28 only
  when a suppression is actually removed (`bridge/tests/complexity_ratchet.rs:77-99`).
- Nested provider-dispatch guard coverage remains green after every move.
- New component APIs do not accept `&mut AppState`.
- Concurrency tests hold each off-lock phase open and prove an unrelated frame
  can still complete before release.

The practical stopping point is an `app.rs` that constructs and coordinates
components, exposes the stable facade, and contains no endpoint implementation,
git/process work, terminal pump, or persistence SQL. Further lock partitioning
should be driven by measured contention after this reorganization, not bundled
into it.
