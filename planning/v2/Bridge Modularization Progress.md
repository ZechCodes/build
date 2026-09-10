# Bridge modularization progress

The remaining work continues from merged commit `738635c0`. Stages 1–2 are
complete: nested source guards and capability-based app tests. That baseline
contains 24,138 lines in `app.rs`, 757 app tests, and 222 shared/helper items.

## Remaining sequence

| Stage | Responsibility | State |
| --- | --- | --- |
| 3 | Move encoding, filesystem-scope, shell, and independent QA utilities to their owners | Complete |
| 4 | Extract conversation application methods and request parsing | Prepared outside repository |
| 5 | Extract project, configuration, and capture application methods | Prepared outside repository |
| 5b | Give project registration, entity binding, and retained paths their own registry | Planned |
| 6 | Extract issue, run, worktree, board, and protocol adapters | Preparing outside repository |
| 7 | Give session registration and spawn reservations their own component | Planned |
| 8 | Give delivery queues, receipts, and in-flight accounting their own component | Planned |
| 9 | Give board attention and caches their own component | Planned |
| 10 | Reverse lifecycle dependencies through owned results and settlement interfaces | Planned |
| Follow-on | Split thread, store, orchestrator, worktree, git GUI, and ADK harness internals | Preparing outside repository |

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
