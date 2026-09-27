# Bridge modularization progress

Stages 3–10 and all six follow-on leaf splits are complete, continuing from
merged commit `738635c0`. Stages 1–2 had already completed the nested source
guards and capability-based app tests. That baseline
contains 24,138 lines in `app.rs`, 757 app tests, and 222 shared/helper items.

## Completed sequence

| Stage | Responsibility | State |
| --- | --- | --- |
| 3 | Move encoding, filesystem-scope, shell, and independent QA utilities to their owners | Complete |
| 4 | Extract conversation application methods and request parsing | Complete |
| 5 | Extract project, configuration, and capture application methods | Complete |
| 5b | Give project registration, entity binding, and retained paths their own registry | Complete |
| 6 | Extract task, run, worktree, board, and protocol adapters | Complete |
| 7 | Give session registration and spawn reservations their own component | Complete |
| 8 | Give delivery queues, receipts, and in-flight accounting their own component | Complete |
| 9 | Give board attention and caches their own component | Complete |
| 10 | Reverse lifecycle dependencies through owned results and settlement interfaces | Complete |
| Follow-on | Split thread, store, orchestrator, worktree, git GUI, and ADK harness internals | Complete |

Each stage is committed after its source review and validation. Mechanical
extraction preserves executable bodies; ownership changes receive separate
review of transaction boundaries, cancellation, and publication ordering.
New component interfaces accept their own state and inputs, not `AppState`.
Existing public paths remain available through facade exports.

Only one integration writer changes `app.rs` at a time. Later stages prepare
disjoint destination modules outside the repository until integration: source
guards also inspect unwired Rust files. Validation and commits cover only the
integrated stage. Project registration
remains publish-after-persistence, while board cache ownership is separated
from project identity in stage 9. Lifecycle reversal includes reservation
rollback and caller continuations, not only its top-level import.

## Validation baseline

Before this continuation, the full Rust run reported 2,184 passed, one failed,
and seven ignored. The only failure was the previously reproduced
`concurrency_load` terminal-output floor. All 2,109 exercised library tests
passed. Formatting, all-target Clippy, Semgrep, and Gitleaks passed. Live
Python interop was not exercised because `BUILD_SECURE_TRANSPORT_PY` was unset.
The release build passed with two existing production-only warnings.

Stage validation records below distinguish this baseline failure from any new
failure; neither runtime behavior nor benchmark thresholds are changed to make
a refactor pass. See [the strategy](Bridge%20Modularization%20Strategy.md) for
the detailed invariants and final ownership boundaries.

## Stage 3

Encoding now lives in `encoding.rs`; canonical and lexical path containment
share `fs_scope.rs`. `screen.rs` and `gitgui.rs` use those owners directly.
The public plan path validator and app shell/environment entry points retain
their original paths through re-exports. Shell discovery and bounded login
PATH capture live in `terminal_environment.rs`. The independent QA file writer
lives in `app/qa.rs`; the six QA lifecycle methods still require application
coordination and will move with the corresponding adapters.

The staged source audit preserves 868 functions in its scoped comparison,
including test attributes and bodies, public function signatures/bodies,
executable literals (including macro literals), and branch counts. Its one
reviewed function difference is the filesystem fence's imported path type
spelling and local call to the same relocated lexical validator. This is a
source preservation check, not a proof of type resolution or state semantics.

All 84 focused tests, all-target check, formatting, and all-target Clippy pass.
The crate-wide source audit preserves all 5,434 functions and every test body;
its two reviewed function differences are the relocated path-validation calls.
Semgrep reports no findings across the ten changed Rust files, and staged
Gitleaks passes. The full suite reports 2,184 passed, one failed, and seven
ignored; all 2,109 exercised library tests pass. The sole failure is the known
`concurrency_load` output floor, again at 75,226 bytes. `app.rs` is now 24,035
lines. No benchmark threshold or runtime behavior changed.

## Stage 4

Conversation address resolution, reading and pagination, posting and receipts,
attachments, and input parsing now live in five cohesive modules under
`app/conversations/`. The largest is 662 lines; `app.rs` is 21,873 lines.
The public `app::ATTACHMENT_MAX_BYTES` path remains available. Cross-record
persistence stays in the application coordinator for the later ownership stage.
Production imports are explicit, and moved internal items remain app-scoped.

The staged crate-wide audit retains all 5,434 functions and the exact test,
literal, branch, and public-function inventories. The declaration audit also
preserves all 1,028 structs, enums, aliases, constants, and statics, including
their attributes and field types. Its eight reviewed function
pairs differ only by trailing commas in signatures wrapped by rustfmt after
the required visibility change; their bodies are unchanged. Focused conversation
tests pass 51 cases with one configured ignore. Formatting, compilation, and
all-target Clippy pass. The full suite reports 2,184 passed, one failed, and
seven ignored; all 2,109 exercised library tests pass. The only failure is the
known `concurrency_load` output floor, at 80,414 bytes. Semgrep finds no issues
across all eight changed Rust files, and staged Gitleaks passes. The complexity
ratchet remains 28; its explanatory comment now survives module relocation.

## Stage 5

Project registration and requests, configuration parsing and persistence, and
capture storage and routing now live in nine modules under `app/projects/`,
`app/config/`, and `app/captures/`. Each module is under 700 lines; `app.rs` is
19,803 lines. This stage moves 100 declarations without changing their bodies.
Public `app::ConfigError`, `app::RoutedCapture`, `app::ProjectAdded`, and
`app::ProjectRemoteSet` exports remain available. Project registry ownership is
a separate next step; configuration persistence order and direct registration
behavior remain unchanged here.

Formatting, all-target compilation, and all-target Clippy pass. All 64 focused
configuration tests and 43 routing tests pass. The full crate audit preserves
all 5,434 functions and 1,028 data declarations, including test attributes and
bodies, field types, literals, and branch counts. Its nine reviewed function
pairs differ only by trailing parameter commas; all bodies are exact. Semgrep
finds no tasks across the ten changed Rust files, and staged Gitleaks passes.
The full suite reports 2,184 passed, one failed, and seven ignored; all 2,109
exercised library tests pass. The sole failure remains the previously reproduced
`concurrency_load` output floor, at 80,414 bytes.

## Stage 5b

`ProjectRegistry` now owns the registered-project vector, live entity bindings,
retained recovery paths, and next project id behind private fields. Registration
still resolves paths and rejects duplicates before building an orchestrator.
RPC registration holds an unpublished candidate through the configuration write,
creates its reply, then publishes and advances the id. Direct registration does
not write configuration. Recovery retains the exact stored path text; paired
binding removal and live-only capture cleanup remain distinct operations.

The registry is 173 lines; `app.rs` is 19,791 lines. Production callers receive
immutable project views and narrow binding/publication operations. Temporary
mutable project access remains for isolation settings and board caches; stage 9
removes cache ownership from Project. Legacy test setup uses test-only indexing,
clear, and id observations without exposing production collections.

Root and independent review found the final production conversion preserves
ordering and JSON shapes. Review corrected an intermediate eager isolation
availability probe to its original non-null branch after parameter parsing.
Existing tests retain their assertions while changing project access paths.
Six registry invariants and all 66 configuration tests pass, including two new
real-adapter tests for direct registration without persistence and missing-path
fallback. Formatting, all-target compilation, all-target Clippy, and diff checks pass.
The structural inventory retains all 757 baseline app tests and adds eight;
all 119 route arms and 13 frame-dispatch arms remain unchanged. The source
inventory reports the expected registry ownership and test-access changes:
5,434 functions become 5,470 and 1,028 data declarations become 1,031. The only
public function-body difference is `add_project` using the equivalent registry
lookup. This stage is an ownership change, so byte-equivalent bodies are not
claimed. Semgrep reports no findings across all 48 changed Rust files.
The full suite reports 2,192 passed, one failed, and seven ignored; all 2,117
exercised library tests pass. The only failure remains the previously reproduced
`concurrency_load` output floor, at 75,226 bytes. Staged Gitleaks passes.

## Stage 6

The remaining application adapters now live under task, run, worktree, board,
protocol, and runtime modules. `app.rs` is 741 lines of composition, construction,
and facade exports; the largest extracted production file is 1,456 lines.
Production imports name their owners explicitly, and the public app paths remain
available. The earlier project, configuration, capture, and conversation modules
are unchanged in this stage. State ownership changes remain separate stages.

The crate-wide staged audit preserves all 5,470 functions and 1,031 data
declarations. All 104 reported function pairs have identical bodies and
attributes; only trailing parameter commas differ after formatting. The separate
structural audit retains all 765 app tests and all 119 RPC route arms and 13
frame-dispatch arms. Compilation, formatting, all-target Clippy, and all 121 focused runtime tests
pass. Test-only imports are scoped to test builds; unused facade aliases were
removed without changing function bodies. The final source audit is identical
to the reviewed report; Semgrep reports no findings across all 43 changed Rust
files. The full suite reports 2,192 passed, one failed, and seven ignored; all
2,117 exercised library tests pass. The sole failure is the previously reproduced
`concurrency_load` output floor, at 77,820 bytes. Staged Gitleaks passes.

## Stage 7

Session registration, spawn claims, waiting screens, MCP capabilities, and
terminal ids now belong to `SessionRegistry` behind private storage. The existing
application mutex remains the only registry lock. Runtime retains weak-handle
claim settlement, conversation updates, exact target validation, and off-lock
provider work. Adapters use owned facts and handles; fixture accessors exist
only in test builds.

Independent production review corrected four intermediate differences: terminal
input and resize probe order, key-selected teardown versus role-selected stale
owners, raw tab state versus provider-ended preflight status, and the original
board projection predicates. The final ownership contract preserves publication,
retirement, token revocation, pump fences, and orphan-reap ordering. Test review
retains all 765 baseline app tests and adds nine registry invariant tests plus
one real preflight regression for an ended native-choice session with a changed
frozen choice. Existing test diagnostics, literals, timing, and assertions remain
preserved through registry fixture operations. `app.rs` is 715 lines; the registry core is 808 lines with 212 lines of
component tests. Formatting, all-target compilation, and all-target Clippy pass.
Focused runs pass nine registry tests, 122 runtime tests, and two MCP tests.
The staged structural audit retains all baseline test names and 132 RPC names;
its ten additions are the new tests. The ownership audit records 5,470 functions
becoming 5,550 and 1,031 data declarations becoming 1,044, with no public function
signature/body differences. This ownership stage intentionally changes internal
bodies and test fixture access, so byte-equivalent bodies are not claimed.
Semgrep reports no findings across all 38 changed Rust files. The full suite reports 2,202 passed, one failed, and seven ignored; all 2,127
exercised library tests pass. The only failure is the previously reproduced
`concurrency_load` output floor, at 80,414 bytes. Staged Gitleaks passes.

## Stage 8

`DeliveryQueue` owns queued turns, refusal checkpoints, and owner/agent in-flight
counts. Runtime retains a separate RAII mark for each popped turn and the weak
application handle used to settle during unwinding. Every ready turn is prepared
before tickets are minted; held and ready order, claim-error requeue, false-claim
settlement, and the single deferred-delivery settlement tail remain unchanged.
`OperationLedger` owns the receipt mirror and pending acceptance. A matching
acceptance stays consumed on both Store success and failure; Store reads and
transitions remain authoritative, and boot restore does not hydrate the mirror.
The accepted-plan attachment operation preserves its exact target and payload.

`app.rs` is 696 lines. The queue and ledger cores are 205 and 90 lines, with
conventional child test modules. Independent production and test review found no
remaining task. All 775 baseline app tests remain, with nine new component
invariants. All 119 route and 13 frame-dispatch names are unchanged. The source
inventory records the expected ownership changes: 5,550 functions become 5,579
and 1,044 data declarations become 1,047; public function signatures and bodies
have no inventory differences. Existing test assertions, diagnostics, literals,
and timing are preserved through the fixture migration. Formatting, all-target
compilation, all-target Clippy, and 13 focused tests pass. Semgrep reports no
findings across all 42 changed Rust files. The full suite reports 2,211 passed, one failed, and seven ignored; all 2,136
exercised library tests pass. The sole failure remains the previously reproduced
`concurrency_load` output floor, at 75,226 bytes. Staged Gitleaks passes.

## Stage 9

The board ownership stage groups attention entries and clocks, notification
watermarks, archive projections, and diff caches behind `BoardIndex`. Store and
filesystem effects remain in application adapters. Refresh claims remain owned
by the existing outer worker across blocking work, with explicit settlement;
the component introduces no cancellation or drop behavior. Project publication
registers its cache slot before board effects, and the existing test-only project
clear operation clears cache payloads without releasing outstanding claims.

Independent production and test review retains exact attention persistence,
notification watermark, archive hydration, scan failure, and claim settlement
boundaries. All 784 existing app tests remain, with 14 new BoardIndex tests.
Test fixture setters retain their original field scope and missing-entry
diagnostics. `app.rs` is 645 lines; the board facade, attention index, and
cache core are 83, 409, and 471 lines. Formatting, all-target compilation,
and Clippy with warnings denied pass. Focused runs pass 14 BoardIndex,
17 branch-feed, and 24 diff-cache tests. The structural audit retains all
132 RPC names and adds only the 14 reviewed tests. Public function inventories
have no differences. The ownership source inventory records 5,579 functions
becoming 5,684 and 1,047 data declarations becoming 1,060. The full suite and
security checks use frozen source. The expanded audit accounts for all 18
baseline-only internal signatures through BoardIndex and claim APIs; existing
literal values and public paths remain preserved. Semgrep reports no findings
across all 38 changed Rust files. The full suite reports 2,225 passed, one failed, and seven ignored; all 2,150
exercised library tests pass. The sole failure remains the previously reproduced
`concurrency_load` output floor at 75,226 bytes. Staged Gitleaks passes.

## Stage 10

Lifecycle disk work returns owned typed facts. The application runtime owns
settlement, callbacks, active entities, publication, and rollback; the lifecycle
leaf no longer depends on application types. Thin compatibility carriers retain
existing public app paths and field shapes while delegating to the same typed
settlement implementation. Ordinary and holding jobs share a neutral test gate.
The fresh replay preserves delivery and board ownership from the preceding
stages. `app.rs` is 640 lines. Independent production review confirms the
12 task/settlement pairings, exact callback and publication ordering, nested
restore failures, and judgment before checkout removal. All 798 baseline app
tests remain; two new tests count real public refusal callbacks exactly once.
The test gate retains its arrival, release, observation, and timeout behavior.

Formatting, all-target compilation, and Clippy with warnings denied pass. Six
method-scoped boxed-receiver lint exceptions preserve the existing public
compatibility signatures; no blanket or cognitive-complexity allowance is added.
The structural audit retains all 132 RPC names. The source audit records
5,684 functions becoming 5,710 and 1,060 data declarations becoming 1,083,
reflecting typed results and app-owned settlements. Semgrep reports no findings
across all 35 changed Rust files. Focused validation passes 58 tests: two
compatibility, 26 off-lock lifecycle, 11 branch-dispatch, and 19 restore cases.
The full suite reports 2,227 passed, one failed, and seven ignored; all 2,152
exercised library tests pass. The sole failure remains the previously reproduced
`concurrency_load` output floor, at 77,820 bytes. Staged Gitleaks passes.

## Follow-on leaf splits

The remaining large leaf files are split behind their existing public facades.
Thread item classification stays canonical for memory and SQLite paging; Store
retains its one connection and transaction owner. Orchestrator keeps caller-owned
active state, WorktreeManager retains the sole backend selector, git discard
keeps its containment fence, and ADK preserves the harness/session boundary.
The six existing facade paths remain stable while their implementations and
420 tests live in conventional capability modules. The combined source retains
5,710 functions and 1,083 data declarations from the completed lifecycle stage.
The 800 app tests and all 132 RPC names remain unchanged. Strict source review
accounts separately for rustfmt punctuation and one call to the existing
`ImplementableTask::slug()` getter in place of direct private-field access;
that getter returns the same borrowed string. No literal or branch inventory
changes result from these moves.

| Facade | Before | After | Child responsibilities |
| --- | ---: | ---: | --- |
| `thread.rs` | 6,552 | 36 | Items, metadata, paging, conversations, rendering |
| `store.rs` | 5,348 | 328 | Schema, entities, conversations, operations, documents, legacy data |
| `orchestrator.rs` | 5,943 | 21 | Plans, runs, reporting, workspace lifecycle |
| `worktree.rs` | 3,747 | 25 | Manager, identity, discovery, comparison, commands, mutation |
| `gitgui.rs` | 2,137 | 24 | History, status, patches, mutations, branches, network |
| `harness/adk.rs` | 4,660 | 157 | Protocol, reader, translation, activity, session |

All newly split leaf files stay below 1,500 lines; the largest is
`worktree/tests/mutation.rs` at 1,314. Existing app test files
`runtime/frame_locks.rs` (1,601) and `workflow/review.rs` (1,592) remain inherited
exceptions. `app.rs` stays at 640 lines.

Independent architecture, source, and test reviews close the ownership and
preservation checks. Formatting, all-target compilation, and all-target Clippy
with warnings denied pass. Focused validation exercises all 416 runnable leaf
tests; four configured ADK tests remain ignored, accounting for all 420 moved
test definitions. Three additional isolation tests selected by the worktree
filter also pass. Semgrep reports no findings across all 94 changed Rust files,
with the final corrected workflow files rescanned. The full suite reports
2,227 passed, one failed, and seven ignored; all 2,152 exercised library tests
pass. All other integration targets and doctests complete. The sole failure
remains the previously reproduced `concurrency_load` output floor, at 75,226
bytes. The recursive provider guard and 28-function complexity ratchet pass.
Staged Gitleaks passes.

## Final state and validation limits

The original 61,274-line application file is now a 640-line facade and
coordinator. Project, session, delivery, operation, and board state have narrow
owners, and lifecycle disk work returns typed results to application settlement
adapters. The original continuation baseline's 757 app tests remain, with 43
reviewed component tests added across the ownership stages, for 800 total.
The six leaf splits preserve all 420 of their existing tests and public facade
paths. Per-stage source reviews distinguish mechanical relocation from
intentional state ownership changes; no single byte-equivalence claim is made
across those ownership changes.

Validation ran locally on macOS. Other supported OS/target configurations were
not compiled here. Live Rust/Python interop was not exercised because
`BUILD_SECURE_TRANSPORT_PY` was unset; the test's early return is not transport
coverage. Configured ignored tests remain ignored. The known concurrency floor
failure is retained with its original threshold. These stages are committed on
the modularization branch; this continuation does not merge or deploy them.
Further lock partitioning, API redesign, performance changes, and unrelated
large-file cleanup are outside the completed modularization plan.
