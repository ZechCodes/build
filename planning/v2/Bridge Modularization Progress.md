# Bridge modularization progress

The remaining work continues from merged commit `738635c0`. Stages 1–2 are
complete: nested source guards and capability-based app tests. That baseline
contains 24,138 lines in `app.rs`, 757 app tests, and 222 shared/helper items.

## Remaining sequence

| Stage | Responsibility | State |
| --- | --- | --- |
| 3 | Move encoding, filesystem-scope, shell, and independent QA utilities to their owners | Complete |
| 4 | Extract conversation application methods and request parsing | Complete |
| 5 | Extract project, configuration, and capture application methods | Complete |
| 5b | Give project registration, entity binding, and retained paths their own registry | Complete |
| 6 | Extract issue, run, worktree, board, and protocol adapters | Complete |
| 7 | Give session registration and spawn reservations their own component | Prepared outside repository |
| 8 | Give delivery queues, receipts, and in-flight accounting their own component | Prepared outside repository |
| 9 | Give board attention and caches their own component | Prepared outside repository |
| 10 | Reverse lifecycle dependencies through owned results and settlement interfaces | Prepared outside repository |
| Follow-on | Split thread, store, orchestrator, worktree, git GUI, and ADK harness internals | Prepared outside repository |

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
finds no issues across the ten changed Rust files, and staged Gitleaks passes.
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

The remaining application adapters now live under issue, run, worktree, board,
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
