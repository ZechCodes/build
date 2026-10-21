# Chat isolation security checklist

**Status:** verified (10/10 controls)

**Scope:** the chat-isolation trust boundary described by
`Chat Isolation Foundation.md`: explicit entity, agent, conversation, session,
turn, draft, and delivery identities across bridge persistence, command
handling, harness lifecycle, and SPA reconciliation.

This checklist is evidence-driven. A control is verified only after its named
tests pass against the completed implementation. Repository scanner results are
recorded from the final uncommitted tree as well as committed history so neither
surface can hide a leak.

| # | Control | Status | Required evidence |
|---|---|---|---|
| 1 | An explicitly supplied chat address is resolved as one `(entity_id, agent_id, conversation_id)` context and fails closed when supplied members are unknown, malformed, or mismatched. Resolvable omissions, including defined legacy commands that omit agent/conversation, derive the recorded binding exactly once at the boundary; they do not authorize later aliasing of an explicit address. | [x] | `resolve_defaults_to_the_first_agent_and_rejects_invalid_explicit_ids`, `explicit_task_post_never_executes_its_implementation_alias`, `thread_post_refuses_terminal_and_unknown_entities`, and the distinct omitted-address task-nudge regression passed. |
| 2 | A canonical shared conversation owns its history metadata, including message seen markings. Model choice, draft state, pending work, operation-consumption ownership, and execution timing remain owned by the addressed agent/controller. | [x] | `a_conversation_binding_does_not_follow_roster_position`, `a_turn_spends_the_agents_own_choice`, `agent_choose_isolated_same_provider_siblings_and_rejects_a_stale_revision`, `a_search_reads_this_agents_conversations_and_no_one_elses`, `operation_reads_are_bounded_to_the_exact_agent_and_payload`, and the SPA agent-rail/chat-repository suites passed. |
| 3 | A session event can affect only its captured agent and session instance; delayed output or EOF from an old instance cannot retire or overwrite its replacement. | [x] | `a_replaced_sessions_late_activity_close_leaves_the_replacement_alone`, `a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone`, `a_late_self_report_never_lands_on_the_session_that_replaced_it`, and `stale_session_end_cannot_clear_its_replacement_execution` passed. |
| 4 | Resume accepts only exact, persisted lineage for the addressed agent and checkout; absent, stale, cross-agent, or mismatched lineage fails visibly or starts fresh without guessing. | [x] | `a_spawn_resumes_exact_lineage_and_never_guesses_from_history`, `agent_history_without_an_exact_name_and_checkout_starts_fresh`, `revived_agents_without_exact_lineage_never_guess_by_checkout`, and `a_terminal_locator_never_authorizes_a_fresh_sessions_resume_identity` passed. |
| 5 | Send freezes its exact execution address, settings revision, draft revision, and operation identity before asynchronous work; every later queue, spawn, and harness-start step validates and uses that captured address and revision, so selection, settings changes, or agent removal cannot retarget it. | [x] | Bridge choice/revision/removal regressions passed, including `choosing_is_agent_owned_and_advances_a_monotonic_revision`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, and the successive/queued frozen-choice harness tests; SPA switch, remount, pending-send, and provisional-creation races passed in the final suite. |
| 6 | Acceptance is distinct from execution and transport recovery. An accepted receipt may remain queued with `operation_error`, but never restores its consumed draft. A claimed operation whose provider handoff cannot be proven after restart becomes `uncertain` under the same durable identity. Retry/status handling is idempotent and cannot duplicate transcript entries or manufacture a new operation. | [x] | `thread_post_receipt_and_message_commit_together_and_retry_is_idempotent`, `boot_requeues_only_safe_intents_and_marks_claimed_handoffs_uncertain`, `failed_operation_acceptance_rolls_back_history_and_can_retry_once`, operation acknowledgement, historical-tail, and SPA operation-error/pruning regressions passed. |
| 7 | Migration preserves each agent's current effective settings and only defensible conversation/session lineage; unknown lineage remains explicitly unresolved. | [x] | `restoring_legacy_settings_freezes_each_agents_effective_choice`, `restoring_a_pre_agent_record_adopts_its_conversation_idempotently`, `v5_database_gains_operation_receipts_without_touching_conversations`, and the full store migration suite passed. |
| 8 | Removing a primary entity or agent never rebinds another controller, draft, send, or session to a different agent or conversation; deliberate retained history remains explicitly addressed. | [x] | `removing_an_implementation_alias_never_rebinds_its_secondary_to_the_task`, `agent_remove_kills_and_reaps_the_agents_live_session`, `a_drained_turn_cannot_spawn_an_agent_removed_before_delivery`, store removal tests, and SPA selection/controller-removal tests passed. |
| 9 | The identifiers introduced or changed here (`operation_id`, provisional creation identity, and execution address members) are strictly validated at their wire and persistence boundaries, and SPA controller/cache ownership remains scoped to the existing account/device context. The existing E2EE authentication boundary is unchanged and is not rescored here. | [x] | `a_creation_operation_is_idempotent_and_cannot_be_reused_differently`, `operation_id_reuse_with_a_different_request_is_rejected`, `operation_reads_are_bounded_to_the_exact_agent_and_payload`, `operation_payload_preserves_batches_attachments_and_normalized_options`, and SPA reconnect/device-scope/provisional-identity tests passed. |
| 10 | Full functional gates and both current-tree and history secret/security scans pass on the completed tree. | [x] | Final bridge format, strict Clippy, check, and test gates; SPA lint, full test, and production build gates; standard and large-file Semgrep; current-tree and full-history Gitleaks all passed as recorded below. |

## Final gate evidence

- Bridge: `cargo fmt --all -- --check`, `cargo check --tests`, and
  `cargo clippy --all-targets -- -D warnings` passed. `cargo test --all` passed
  2,129 tests with zero failures. Seven tests were intentionally ignored: six
  library tests and the opt-in real-store migration fixture, which requires
  `BUILD_MIGRATION_FIXTURE` to name a copy of a real store.
- SPA: `npm run lint -- --no-cache` passed; `npm test` passed 194 files and
  3,363 tests; `npm run build` completed successfully with existing advisory
  bundle/CommonJS warnings.
- Semgrep: the standard `semgrep --config auto .` scan found zero findings in
  494 tracked targets. The supplementary
  `semgrep --config auto --max-target-bytes 10000000 .` scan included the four
  large tracked files, ran 510 rules against 498 targets, and found zero
  findings. One rule timed out on the unrelated
  `planning/v2/Build Landing Page v2.html`; approximately 99.9% of lines parsed.
- Gitleaks: `gitleaks dir --no-banner --redact .` scanned 29.03 MB of the final
  current tree with no leaks. The full-history scan covered 1,653 commits and
  34.84 MB with no leaks.

**Final score: 100/100.** This score applies to the ten scoped chat-isolation
controls above and records the explicit ignored-test and scanner limitations;
it is not a claim that every optional external fixture was executed.

## Merge revalidation — `origin/main`

The completed chat-isolation tree was revalidated after merging the 15 incoming
commits from `origin/main`. The merge combined explicit agent isolation with
upstream per-message read reporting; the focused
`per_message_read_reports_preserve_explicit_agent_isolation` regression passed
before the full gates.

- Bridge: `cargo fmt --all -- --check` and
  `cargo clippy --all-targets -- -D warnings` passed. `cargo test --all` passed
  2,133 tests with zero failures. Seven tests remained intentionally ignored:
  six library tests that require real provider/network conditions and the
  opt-in real-store migration fixture requiring `BUILD_MIGRATION_FIXTURE`.
- SPA: `npm run lint -- --no-cache` passed; `npm test` passed 203 files and
  3,566 tests; `npm run build` completed successfully.
- Semgrep: `semgrep --config auto --max-target-bytes 10000000 .` ran 510 rules
  against all 503 tracked targets and found zero findings. One rule timed out on
  the unrelated `planning/v2/Build Landing Page v2.html`; approximately 99.9%
  of lines parsed.
- Gitleaks current tree: 29.15 MB scanned, no leaks found. Gitleaks history with
  `--full-history HEAD origin/main` covered both merge parents, 1,668 commits and
  35.40 MB, with no leaks found.

**Post-merge score: 100/100.** The same scoped controls remain verified on the
resolved merge tree; the ignored-test and scanner limitations above remain
explicit.

## Follow-up toolbar merge revalidation

After commit `9b5e69ba`, the one additional SPA toolbar commit from
`origin/main` was merged cleanly and reviewed without Bridge changes. SPA lint,
203 files / 3,573 tests, and the production build passed. Expanded Semgrep ran
510 rules against 503 tracked targets with zero findings (the same unrelated
landing-page HTML rule timed out; approximately 99.9% of lines parsed).
Gitleaks found no leaks in the 29.16 MB current tree or in the two-head
`--full-history HEAD origin/main` scan of 1,669 commits / 35.41 MB.

**Follow-up score: 100/100.** The toolbar-only follow-up introduced no change to
the previously verified Bridge isolation controls.

## Conversation reset — #358

This extension covers `conversation.reset` and retirement of the preceding
thread generation. The existing authenticated device/E2EE boundary still
authorizes bridge RPCs. A required `project_id` additionally checks that both
the addressed owner and the canonical conversation belong to that exact
project; it is not a new project-scoped authentication credential.

| # | Reset control | Evidence |
|---|---|---|
| 1 | Validate the project, entity, agent, canonical conversation and expected generation before changing history or processes. | Foreign-project/agent and stale-generation reset regressions; typed API contract and capability fixtures. |
| 2 | Preserve agent identity, names, membership, task assignments and trackers while replacing history for every bound alias. | Identity/settings and assigned/tracked-task reset regressions; shared aliases expose the same canonical generation revision. |
| 3 | Reserve the conversation, stop and reap only its processes outside the app lock, and revoke their queued work and MCP authority. | Established-turn stop and delivery-ticket settlement, post-admission handoff refusal, off-lock reset reservation and live-session retirement regressions; old MCP done/send and deferred task-handoff admission regressions; current internal wake and stale restart-roster tests. |
| 4 | Remove exact provider artifacts and compaction sidecars without deleting another conversation's lineage. | Seventeen native-cleanup tests cover Claude/ADK, Codex/app-server, Pi, canonical/encoded checkout keys, carrier families, symlink refusal, stopped PTYs and reversible staging. |
| 5 | Commit history, operation receipts, upload ownership and readings together; a pre-commit failure restores staged files and queued delivery. | Persisted-history/restart and store-refusal rollback reset regressions. Post-commit cleanup never restores retired artifacts. |
| 6 | Delete exclusively owned attachments, including unsent uploads, while preserving surviving conversation/task references. | Draft-upload/shared-sibling reset regression and reference checks repeated at commit. |
| 7 | Prevent retired pages, activity bodies, drafts, journals, revision bodies and attachment previews from becoming readable or writable again. | Generation, cross-tab UI purge, cached-body lifetime and content-reset suites, including late reads and delayed writes. |
| 8 | Admit rows, surfaces and pooled summaries with their canonical generation in the same cache transaction; scrub embedded feed copies. | Cross-client admission, session-list and feed-reset suites, including alias choice-revision independence, an old pending list reply and legacy embedded transcript copies. |
| 9 | Gate the menu from cached capabilities and preserve the old settings until the final reset action; a fresh project agent gets its standing instructions. | DOM picker/cancellation/refusal tests, Chromium harness-switch/reset flow and fresh project scaffolding regression. |
| 10 | Verify the completed change with the bridge/SPA gates and security scans. | Final gate evidence recorded below. |

### Reset gate evidence

Verified after rebasing onto `cad6ed45`, with `conversation.reset` on wire
3.11.0. The previous-release manifest is generated from upstream 3.10.0;
`fs.projectSources` and `tasks.bodyPrecondition` retain their earlier contracts.

- Bridge: `cargo test`, `cargo clippy --all-targets -- -D warnings` and
  `cargo fmt --check` passed. The full test gate passed 4,004 tests with zero
  failures. Twelve opt-in tests remained ignored: five real-provider turns,
  one fixture recorder, two installed-CLI probes, two timing checks and two
  real-store migration fixtures.
- SPA: `npm run lint`, `npm test` (548 files / 8,607 tests, zero failures or
  unhandled errors) and `npm run build` passed. Chromium checks cover the
  keyboard/phone menu with Clear below the compaction slider, confirmation
  and picker cancellation, harness switch,
  cleared transcript/readings and empty composer in place. The four review
  screenshots were visually checked. The build retained its existing
  libsodium/module and bundle-size advisories.
- Semgrep: `--config auto --error` ran 200 rules on all 48 changed tracked
  JavaScript, MJS and CSS targets, with zero findings and approximately 100%
  parsed lines.
- Gitleaks: the task history (`origin/main..HEAD`) and an archive of the complete
  tracked tree passed with zero findings. Five exact current-tree fingerprints
  now recognize the existing public `diff_key`/`if_diff_key` fixture values;
  these name content rather than granting access, like their already recorded
  history fingerprints. `git diff --check origin/main..HEAD` also passed.
- Every gate ran under `nice -n 10` with all inherited `BRIDGE_*` variables
  removed. No serving or pairing bridge process was started for this work.

**Reset score: 100/100.** All ten reset controls are verified by the completed
gates above. The existing authentication boundary and the explicit opt-in test
and scanner scopes remain as described here.


## #522 extension — verified by C10 / #538

These controls follow [Project workspace](Project%20Workspace%20Spec.md).
The checked rows have combined C10 / #538 evidence at commit
`6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. The earlier scoped checkpoints below
record the evidence available at those checkpoints; the final C10 entry closes
their pending combined acceptance evidence.

| ID | Control | Verified | Passing evidence |
| --- | --- | --- | --- |
| CI522-1 | One persisted coordinator survives migration, reset and repeated ensure; no per-agent surface may remove, transfer or replace the pin while the project exists. | [x] | `project_roster_migrates_one_stable_coordinator`, `project_coordinator_cannot_be_removed_or_reassigned`, `old_clients_see_only_the_pinned_coordinator`; `accepts the combined original-source / worker / console / child journey (390px)` and `accepts the combined original-source / worker / console / child journey (1280px)`. Full bridge/SPA gates exit 0 at `6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. |
| CI522-2 | Worker prompt/tools use persisted purpose; exact agent/conversation/session generation gates delivery and MCP, independent of prefix, roster position or forged surface flags. | [x] | `project_worker_tools_follow_persisted_purpose`, `a_worker_cannot_administer_sources_or_retire_the_pin_through_mcp`, `a_stale_worker_session_cannot_assign`, `stale_project_sessions_cannot_act_for_the_pin_or_a_removed_worker`; `names a worker through Add and sends only to each exact project conversation`; `paints a cold cache, reconnects without losing source or agent drafts, then safely downgrades`. Full bridge/SPA gates exit 0 at `6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. |
| CI522-3 | Shared cwd never aliases resume/reset/remove artifacts; deleting one worker cannot affect siblings, pin, source files or another draft. | [x] | `project_worker_removal_preserves_shared_cwd_siblings`, `a_spawn_resumes_exact_lineage_and_never_guesses_from_history`, `project_workspace_combined_journey_and_source_matrix`; `accepts the combined original-source / worker / console / child journey (390px)` and `accepts the combined original-source / worker / console / child journey (1280px)` (worker removal retains sibling/pin conversations, sibling draft and source commit draft). Full bridge/SPA gates exit 0 at `6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. |

C5 evidence (#533; the rows stay pending until C8/C10 add theirs):

- CI522-2: `a_stale_worker_session_cannot_assign` (an MCP frame admitted
  before the worker's reset is refused), `a_worker_cannot_administer_sources_or_retire_the_pin_through_mcp`
  (persisted purpose, not the launch declaration, gates the project tools).
- CI522-3: `project_worker_removal_preserves_shared_cwd_siblings` (three live
  sessions in the project's folder; removing one worker and resetting another
  leave the pin's session, token, history and launch config and an
  uncommitted source edit untouched).

### C8 / #536 scoped verification — 2026-10-13

Fix-round checkpoint: findings 1–6 and copy have focused passing evidence.
The full SPA gate pass and final scans remain pending at the mandatory
context-limit handoff; this entry makes no final verification claim.

The SPA contribution verifies the following controls; final fix-round
revalidation and the combined #522 extension's C10 evidence remain pending.
These results supplement existing C4/C5 bridge authority and shared-cwd
cleanup evidence, without rescoring those implementations.

| Control | C8 evidence | Scoped score |
| --- | --- | ---: |
| CI522-1 | Rail model/DOM tests and the worker-first Chromium fixture keep the persisted pin above the separator, refuse its removal, and exclude project workers from named-workspace rails. Project-hosted tasks retain the project rail. Complete-roster Overview and lineage tests follow roster/support changes and retain unwatched workers and their descendants. | 25/25 |
| CI522-2 | Exact worker conversation sends, independent drafts, cached support downgrade, same-owner/device isolation and retired-reader tests. Generation-admission regressions pause a roster read across a real reset: both absent and present initial thread records refuse the retired digest and admit a fresh retry. Roster and captured thread writes are fenced in one transaction. The scheduler bounds unrelated roster requests at two while selected thread/Git/task recovery and a queued owner continue. | 25/25 |
| CI522-3 | Resetting one shared-owner worker invalidates its captured submission while sibling and pin drafts survive. DOM removal and Chromium navigation preserve separate drafts and the canonical pin conversation across surfaces. At 390/1280 widths, project → workspace A → workspace B → project transitions retain the exact composer/body/thread nodes, visibility and focus throughout synchronous, mutation and animation-frame samples; sends retain the exact canonical pin address and current viewing context. Task lines and rail notices use shared purpose-aware identity: named Codex workers retain their cached name and own harness mark, while the coordinator keeps the project mark. Provider/source preservation remains covered by inherited C5 evidence. | 25/25 |

Focused gates ran under `nice -n 10`: rail DOM/isolation and shell project/link
suites passed 385 tests (exit 0); `browser/projectWorkspaceAgents.test.js`
passed all four Chromium tests (exit 0). Cache/reset/activity regressions
passed 29 tests (exit 0), after both new concurrent-reset regressions failed
first (exit 1). The route scheduling regression failed first (exit 1), then
passed all ten route/rail-reading tests (exit 0); it restores independent
parallel route-cache reads without weakening assertions. Scoped source lint
passed (exit 0). Final broad gates and scans are recorded on #536 when complete.

Fix-round evidence for findings 1–4: Overview failed first and then passed
22 tests; lineage failed first and then passed 41 lineage/model tests; removed
worker identity failed first and then passed 37 task identity/notice tests,
then 38 after the legacy fallback regression. Durable identity client/contracts
passed 71 tests; the API contract regression failed first and then passed all
46 contract tests. The roster scheduler regression failed first and then
passed; its related six suites passed 258 tests. These focused runs used
`nice -n 10` and the green results exited 0. The legacy project-purpose
metadata regression failed first (exit 101), then 11 bridge identity tests
passed (exit 0): live and stored project purposes clear synthetic workspace
fields, including fields preserved in older identity snapshots. Finding 5's
390/1280 Chromium regressions failed first against the prior production code
(exit 1), then all seven worker Chromium tests passed (exit 0); nine focused
rail/shell/browser suites passed 452 tests (exit 0) and scoped lint passed
(exit 0). The test checks the same visible, focused nodes at every transition
step, including transient mutation/frame states. Finding 6's task-line/notice
regressions failed first (seven semantic failures, exit 1); the cached rail
notice regression also failed first. Three notice/task-line/rail suites then
passed 403 tests (exit 0), and scoped lint passed (exit 0). Project-worker
fixture cases cover named Codex workers, durable removed workers, cached bare
actors/assignees and the coordinator's project mark. Copy/cache notice
regressions failed first (exit 1), then the focused project/pin suites passed
102 tests (exit 0) and scoped lint passed (exit 0). Cached capable →
unsupported → capable transitions preserve the same composer/thread and focus;
unknown support and workspace pins remain silent. Final broad gates remain
pending in this fix round; earlier gate results above do not certify the
final revision.

Together with TS522-1's 25/25 recorded in the Task checklist, the four applicable
C8 controls score **100/100 within C8's implemented scope**. This is not a
completion claim for the combined #522/C10 extension.

### C10 / #538 combined verification — 2026-10-13

All 3 controls above are verified at
`6b6a236e2b4e51ba4f6d0b8ff3c924100912a687`. Full bridge `cargo test` exited 0,
including `project_workspace_combined_journey_and_source_matrix`, coordinator,
persisted-purpose, exact-lineage, stale-generation and lifecycle regressions.
The run used the real uv 0.12.15 executable through `BST_UV` and `PATH`, with
`$HOME/.cargo/bin` first. Full SPA `npm test` under Node 22.23.3 / npm 12.0.2
and `CHROMIUM_PATH=/usr/bin/chromium` exited 0: 669 files / 10,473 tests passed,
including all seven `browser/projectWorkspaceFlow.test.js` cases. The two full
390/1280 journeys retain exact worker/task addresses, sibling and source drafts,
and the canonical pin across source, console and child-workspace navigation.

Semgrep `--config auto --error` on every changed file, Gitleaks with
`--log-opts='origin/main..HEAD'`, and `git diff --check origin/main..HEAD` each
exited 0. All gates ran under `nice -n 10` with output redirected to logs.
The four-pair compatibility matrix uses frozen legacy requests, the production
client adapter and isolated old/new protocol fixtures; it does not claim
historical executable runs. #538 records the gate exits and final publication.

## #652 scoped compaction policy storage — 2026-10-18

This extension covers storage, migration and configured-policy resolution only.
It does not certify native harness enforcement, active-session observations or
the later per-harness client API.

| Control | Evidence | Score |
| --- | --- | ---: |
| Independent harness and owner scopes cannot overwrite one another. | The five-provider precedence/clear matrix, two-project/workspace isolation and project-worker scope tests pass. | 20/20 |
| New values are positive safe integers; legacy values are preserved without rounding. | Invalid/mixed patches, numeric boundaries, exact unsafe legacy observations and explicit-empty-map restart tests pass. Legacy zero normalizes in the live scalar, settings/digest/context reads, compaction checks and persisted reloads; repeated saves are idempotent. | 20/20 |
| Persistence failure cannot publish a partial policy mutation. | Write and rename fault tests preserve disk and memory; legacy scalar broadcasts are atomic and leave narrower scopes alone. | 20/20 |
| Shared history owns one override while the addressed launch provider selects its harness entry. | Same/different-provider aliases share canonical scopes and overrides; alias writes notify both addresses. Active and parked historical owners cannot inherit a reused workspace's policy. | 20/20 |
| Lifecycle changes preserve source files and remove settings only after confirmed deletion. | Rename/restart, parked revival, stable-ID adopted restart, deletion preflight and retained-cleanup/retry tests pass. Missing and permission-denied adopted checkouts retain policy and recover it on reappearance. The source stays untouched, and the permission test restores the exact original mode during teardown. | 20/20 |

All required checks ran under nice 10. The full, unfiltered `cargo test`
passed 4,995 tests with zero failures. Thirteen existing opt-in tests remained
ignored. All normal integration suites ran, including both CLI-version tests
against the freshly built binary. Strict all-target
Clippy and formatting passed. The locked vendor library gate passed 223 tests
with four existing ignores. Semgrep ran 51 rules on all 26 changed Rust files,
with zero findings; one worker avoided a host resource-allocation error.
The required Gitleaks `main..HEAD` history scan passed.

The scope and code-review regressions failed before their fixes: legacy zero
runtime/read behavior, missing-checkout retention and an actual EACCES from
denied parent-directory traversal. The full rerun includes all three fixes.
Missing-checkout retention also survives an unrelated config save and restart
before the checkout returns. Pruning no longer attempts durable policy cleanup;
only explicit, confirmed deletion does.

The supplementary whole tracked-tree Gitleaks scan exited 1 for two unchanged
public transport-vector findings at `transport/fixtures/v1.json:308` and
`:310`. Their public ciphertext/session-key provenance and exact introducing
commit exceptions are already documented in `transport/fixtures/README.md`
and `.gitleaksignore`. No scanner exceptions were added by this task.

Each gate used a throwaway HOME under /tmp, deleted on completion. Existing
Cargo/Rustup directories were mounted read-only, Cargo's bin directory came
first on PATH, and the Python interop test used the existing uv executable via
`BST_UV`. Inherited BRIDGE variables were left unchanged.

**Scoped score: 100/100.** The existing authentication boundary and public
transport fixtures are not rescored. Confirmed deletion can retain an orphan
policy if its final config write fails; a same-path adopted checkout can then
reuse that policy. Adoption re-identification and migration remain #654; this
task does not change adoption order or IDs.

## #696 agent message attachments — 2026-10-20

Agent replies and handoffs now carry durable attachment descriptors using the
existing session, address and generation ownership checks. Their shared file
intake, persistence, reset-reference and bounded-read verification is scoped in
[Agent Message Attachments Security Checklist](Agent%20Message%20Attachments%20Security%20Checklist.md).
This extension does not change the authentication or reset deletion controls
scored above.

## #740 authenticated shell account identity — 2026-10-21

This section covers only the server-supplied account identity used to scope
the SPA's local records. Pending write reconciliation and client-side identity
admission are verified separately; this metadata does not authorize an API call.

| Control | Evidence in `skriftapp/buildapp/test_app_shell_cache.py` | Score |
| --- | --- | ---: |
| The browser shell names the authenticated session's parsed UUID; request query, header and competing request-state values cannot select another account. | `test_the_shell_names_only_the_authenticated_account` | 20/20 |
| Missing or malformed browser authentication redirects before rendering an account-bearing shell. | `test_a_signed_out_request_carries_no_account_id`, both cases | 20/20 |
| Only the exact `{{ACCOUNT_ID}}` placeholder is replaced, with HTML attribute escaping. | `test_the_shell_names_only_the_authenticated_account`, `test_the_account_placeholder_is_encoded_as_an_html_attribute` | 20/20 |
| The SPA source declares the `build-account-id` meta field, and the existing application mount survives rendering. | `test_the_spa_source_declares_the_account_placeholder`, `test_the_shell_still_carries_the_app` | 20/20 |
| Account-bearing shell responses are never cached. | `test_the_app_shell_is_never_cached` | 20/20 |

**Scoped shell score: 100/100.** The seven focused tests passed after the
three new rendering/template assertions failed first. The literal placeholder
in an unrendered development template is unavailable account identity, not an
account id. Production rendering also retains the guarded desktop route's
existing authenticated-user resolution.

The full skriftapp gates passed under `nice -n 10`: frozen Ruff (exit 0) and
frozen pytest (957 passed, one existing skip, exit 0). Both used the real uv
0.12.15 executable and the dedicated `/tmp/build-740-sk-venv` environment.
Logs: `/tmp/build-740-sk-ruff.log` and `/tmp/build-740-sk-pytest.log`.
Semgrep scanned the changed controller, tests and HTML template with 304 rules,
zero findings and approximately 100% parsed lines (exit 0;
`/tmp/build-740-sk-semgrep.log`).


## #740 pending-write and epoch boundaries — 2026-10-21

This extension covers the SPA side of wire 3.24.0. The bridge enables each
semantic method through its greeting allowlist; the foundation enables none.
Bridge journal execution and paged result references are outside this scope.

| Control | Evidence | Score |
| --- | --- | ---: |
| Account, paired device and principal isolate durable intent and its visible layers; missing authenticated shell identity cannot select a namespace. | `writeOperationCache.test.js` namespace/deactivation tests and `writeOperationRuntime.test.js` authenticated metadata tests. | 10/10 |
| A durable send cannot precede committed intent, and storage failure cannot claim acceptance. | `writeOperations.test.js` persistence-before-transport and refused-storage tests; cache transaction rollback preserves the original provisional row. | 10/10 |
| Genuine durable ack is success; a receipt, timeout, cancellation or lost carrier cannot invent success or delete intent. | `sessionRpc.test.js` receipt/ack/late-frame tests and `writeOperations.test.js` local-cancellation and same-ID recovery tests. | 10/10 |
| Current greeting authority fences every application and terminal frame before correlation, credits or cache mutation. Uncorrelated epochs cannot establish authority. | `bridgeEpoch.test.js`, `sessionRpc.test.js` preauthority/mismatched-epoch tests and terminal epoch regressions. | 10/10 |
| Journal changes and acknowledged-but-missing records remain unresolved without replay; unknown result references cannot resolve an action successfully. | `writeOperations.test.js` journal quarantine, missing-record and result-reference/expired-result tests. | 10/10 |
| Cross-tab transport uses an expiring lease, renewed through asynchronous handoff and status retries; duplicate callbacks cannot regress durable revision. | Cache lease/transaction tests, `writeOperationWorker.test.js`, delayed-handoff and reordered-frame service regressions. | 10/10 |
| Ordered layers use owner kind, ID and generation, remain separate from canonical writes, and retire only against matching publication evidence. | `writeOperationCache.test.js` generation, owner ordering, canonical mutation and complete-publication tests. | 10/10 |
| Publication cannot erase accepted task floors or permit an older detail read to overwrite a confirmed comment. | Atomic publication/read-note tests in `writeOperationCache.test.js` and captured-base/detail-basis tests in `writeOperationRefresh.test.js`. | 10/10 |
| Draft recovery and named refresh obligations survive prehello queuing and reload; failure removes only its own unapplied layer. Unknown refresh domains stay unresolved. | `prehelloWriteActions.test.js`, `taskCommentRecovery.test.js`, layer rollback and persistent refresh-obligation service tests. | 10/10 |
| A new bridge process resets terminal cursors and acknowledgements, reattaches desired terminals, and never replays input. | `terminal.test.js` same-carrier boot change and `terminalReconnectIntegration.test.js` real-manager remint/reattach regression. | 10/10 |

**Scoped pending-write score: 100/100.** The focused cache, service,
effects, refresh, runtime, RPC and terminal run passed 221 tests (exit 0).
The prehello, stale task-read and same-carrier terminal regressions failed
before their fixes. A further empty-overlay refresh regression failed first,
then 89 cache/service/refresh tests passed after inactive empty layers stopped
holding a completed refresh open. Full merged-main SPA gates remain the task's handoff gate;
this scoped score is not a claim that those gates have already passed.
