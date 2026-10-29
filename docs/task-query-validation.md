# Task #900 validation record

The task branch starts at `12b8f869bd94e259666df69f1a8c0cde5c695982`.
Search remains explicitly excluded from implementation; see
[the matching comparison](task-search-options.md). Performance measurements and
their limits belong in [task-query-performance.md](task-query-performance.md).

## Preserved failed candidates

Independent final Sol scope and separate Sol code reviews both requested changes
on `f7f2d68031d95c868df0a209cea85a9a283a1823`. This was not an approval.
The code finding was selector errors classified as internal rather than invalid
parameters. Scope findings required executable joint static-stream, attention
read/watch/dismissal/bookkeeping, and review-mode exclusion evidence. Descendant
tests and fixes must receive new exact-head reviews.

The first full `nice -n 10 cargo test` compiled the clean f7f2d680 candidate.
It exited **101** after **5,968 passed, 1 failed, 24 ignored** unit tests;
integration execution did not proceed. The failure was
`api::v1::tests::no_family_handler_signature_mentions_value`: the new concrete
`nonnull_value` deserializer returned `Option<Value>`. The descendant fix uses
a generic non-null deserializer without weakening the architecture test.
Review fixes were being prepared while that run executed; this failed run is
not credited to their descendant heads. The original log is
`/tmp/task900-cargo-full.log`.

Earlier integrated `query_` validation returned **101** with **33 passed,
1 failed** because its plan assertion rejected SQLite's stronger `USING COVERING
INDEX` wording. The corrected assertion requires the named indexed project
SEARCH and rejects a table scan. Its failed log remains
`/tmp/task900-query-final.log`.

The first review-fix `query_` run returned **101**, **40 passed, 1 failed**.
All three new RPC behavior tests passed; an attention fixture incorrectly tried
to transition blocked work into review, and the supported graph producer
correctly rejected it. The fixture must remove that dependency before testing
the independent legacy review reason. This log remains
`/tmp/task900-review-fixes-query.log`.

The explicit-import attention regression then returned **101**, **3 passed,
1 failed**: an unknown `notifies_user` property on a raw Created event was
counted by the inherited summary SQL even though canonical `TaskEvent` decoding
does not retain that property. Canonical event asks use `mentions_user`;
comment notifications remain supported. The regression log is
`/tmp/task900-attention-red.log` and must remain with the corrected result.

## Withdrawn Astra candidate and expanded source audit

Independent Astra reviewer `/root/astra_final_review` (gpt-6-astra) requested
changes on `0d3e81643f66846db76258809ff4888a1c8a3d76` against the exact base.
The handoff was withdrawn. Both P2 findings concern new query facts consuming
lossy display inputs: SQLite text `substr` stops at NUL, allowing unsupported
canonical timestamps to become supported prefixes, and changing opaque assignee
IDs. A leading-NUL assignee became the empty project bucket and could contribute
twice. Existing successful gates do not approve a production repair descendant.

The expanded source audit also covers status/labels, read/watch/dismiss markers,
history-ID suffixes and pending-target presence/kind. See
[the source-to-predicate map](task-query-source-audit.md), including safe-source
inspection, implementation boundaries and regression ownership.

New tests against the pre-fix production implementation observed these failures:

- Timestamp regressions: exit **101**, **0 passed, 3 failed**,
  `/tmp/task900-raw-timestamp-red.log`.
- Expanded timestamp/structural-source regressions: exit **101**,
  **0 passed, 5 failed**, `/tmp/task900-exact-source-red.log`.
- Actual RPC exact-text regressions: exit **101**, **0 passed, 2 failed**,
  `/tmp/task900-query-exact-text-red.log`.
- Expanded marker/fact-ID/pending-kind source regressions: exit **101**,
  **0 passed, 8 failed**, `/tmp/task900-all-source-red.log`.

The first integrated repair attempt also exited **101**, **10 passed, 1 failed**
(`/tmp/task900-all-source-green.log`, despite its planned-green filename). The
canonical null/empty oracle caught a real helper bug: SQLite BLOB `substr` of an
empty value returns NULL, changing a present empty watch floor into absence and
incorrectly applying fallback. The suffix helper needs an explicit zero-byte
empty case; leading NUL must remain nonempty by BLOB length.
A follow-up compilation (`/tmp/task900-all-source-green2.log`) was deliberately
terminated with exit **143**, before test execution, to cover the equivalent
trailing-hyphen empty-suffix edge as well. It is not a test pass. The final
helper preserves NULL input separately from an empty result after the first
hyphen.

These RED builds included new regression additions to the working tree; they
are not clean-head full-suite executions. Repairs must classify canonical bytes
before display reduction, preserve storage/legacy parsing, maintain structural
facts on supported producers and invalidate prior derived rows on upgrade.

The lead explicitly authorized project-scoped, same-basis refusal for overlong
stored read/watch/dismiss markers in whole counts responses and attention list
queries. This preserves write acceptance and exact markers up to 128 UTF-8
bytes, including NUL, without claiming complete bounded support for longer
historical markers. Canonical Some("") remains distinct from None; this differs
from current SPA helper truthiness. #891 requires separate current-consumer
versus canonical behavior evidence and review before activation. No legacy
helper parity or consumer activation is inferred from the backend profile.

The fixed integrated source then passed **11/11** source regressions, **6/6**
actual RPC cases and **79/79** full projection cases, all exit **0**. Logs are
`/tmp/task900-all-source-green3.log`, `/tmp/task900-all-source-rpc-green.log`
and `/tmp/task900-all-source-projection-green.log`. These ran the same debug
`build_bridge-0521f5ae737d1028` executable, SHA-256
`edf81e7cb31b3a9314d52e4e3e3cd3e3e914e27e6f21268df346633d9095a1fe`.
It was compiled from the integrated working source subsequently committed as
`eca9e4692` and `f2769d34ddf9cfb9614a6b2d475b6b2f992d7f32`; these focused runs
are not relabeled as clean-head full-suite executions. Fresh committed-head
full Cargo, lint, scans, release timing and independent expanded-delta reviews
remain required. The Astra blockers are not closed by these focused passes.

## Earlier focused evidence

- Integrated projection/producer/import/repair suite: **63 passed**, exit 0,
  before the final review-fix attention expansion.
- Shared API contract suite: **55 passed**, exit 0, at wire commit 874fc1cc5;
  later refusal metadata and selector changes require descendant validation.
- Structured timestamp refusal unit: **1 passed**, exit 0.
- Timestamp profile: standalone Rust tests passed after observed initial stub
  and exactly-three-fraction failures. Node 22.23.4, Node 26.12.1 and Chromium
  152.0.7977.82 matched all 31 supported shared instants. These are measured
  runtime witnesses, not arbitrary-browser proof.
- Initial explicit-path Semgrep scan at f7f2d680: exit 0, 27 files/203 rules,
  zero findings. An initial metrics-off/auto-config invocation exited 2 and
  was corrected. New descendant files require another scan.
- Initial Gitleaks scan at f7f2d680: exit 0, 10 commits, no leaks. Final push
  requires a scan including all descendant commits.

## Integrated fixed candidate and test-only descendant

The clean full run `nice -n 10 cargo test -- --nocapture` on
`622f449bffadef4b2f3046b61288c53d2a307ff9` exited **0**. It passed **5,978**
unit tests with 24 ignored, all normal integration suites (including **55** API
contracts and three timestamp-profile tests), and doc tests. The full log is
`/tmp/task900-cargo-final-fixed.log`. The named
`old_event_ask_projection_rebuilds_all_derived_layers_without_editing_canonical_bytes`
test is included through ancestor `685e9b040` and passed in that run. It seeds
an older, incorrect projection and fingerprints, then verifies repaired facts,
context, card and query attention flags are all false, the projection basis
advances, and canonical task/event bytes are unchanged.

The subsequent full Clippy run at **622f449b** exited **101** for cognitive
complexity in three test functions; its original log is
`/tmp/task900-clippy.log`. Commit
`2c2b3f31fbcd797681747af6f0f16e603085455d` only extracts helpers and splits one
fixture test in those three test files. It preserves assertions and leaves
production source and the upgrade convergence regression untouched. No lint
suppression or ratchet change was introduced. The **622 full-suite result is
not attributed to 2c2**.

At **2c2b3f31f**, Clippy with all targets and denied warnings exited **0**,
format checking exited **0**, and explicit-path Semgrep exited **0** (29 files,
203 rules, zero findings). Both separate final Sol reviewers inspected the
complete base-to-2c2 implementation and the extraction delta: scope reviewer
`/root/final_scope_review` and code reviewer `/root/final_code_review` approve
inspection against the exact base above, conditional on the remaining gates.
These independent final reviews are distinct from preliminary design review.

Affected tests at **2c2b3f31f** all exited **0**:

- Actual RPC selector/agent-stream/review-mode behavior: **3 passed**.
- Complete API contracts after fixture-test split: **56 passed**.
- Aggregate/lookup default fixture: **2 passed, 1 ignored**; the ignored test is
  the separate release timing gate, not a failed correctness check.
- Named four-layer upgrade convergence regression: **1 passed**.

Logs are `/tmp/task900-extraction-rpc.log`, `-api.log`, `-query-fixture.log` and
`-convergence.log` under the same prefix. The release build of both carrier test
executables at this head exited **0** before measurement. Both direct-executable
isolated release runs then exited **0**, serially after the other owned gates
stopped: summary/filter choices (1,200 requests, maximum 81.353 ms, maximum
reported result 340,012 bytes), then aggregate/lookup (600 requests, maximum
1.861 ms, maximum reported result 7,758 bytes). Each sample met its latency and
result-byte limits. Commands, executable identities, every scenario and byte
semantics are in [the performance record](task-query-performance.md).

This ledger pins execution to its source head. The final task #900 report
identifies the documentation-only descendant, final pre-push scan and independent
scope, code and Astra dispositions. Documentation review does not relabel the
622 full-suite run or the 2c2 affected-test and release evidence.
