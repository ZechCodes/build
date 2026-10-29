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


## Canonical-source repair: committed-head gates

Fresh `nice -n 10 cargo test -- --nocapture` on clean committed
`cf06f73af8b94ee30d4671043fb35bc2a30cd930` exited **0**. The log
`/tmp/task900-cargo-canonical-full.log` records that source head, command and
final `process_exit=0`: **5,995** unit tests passed, 24 ignored, **56** API
contracts passed, all normal integration suites passed, and doc tests passed.
This includes both upgrade convergence regressions, all canonical-source and
RPC cases, and the existing producer/legacy coverage. It supersedes earlier
full-suite evidence for the repaired implementation without erasing it.

Clippy (`nice -n 10 cargo clippy --manifest-path bridge/Cargo.toml --all-targets
-- -D warnings`) exited **0** for committed production/test source
`f2769d34ddf9cfb9614a6b2d475b6b2f992d7f32`; the later `cf06` commits only add
audit/validation documentation. The distinction is intentional: no later code
is being credited to an earlier lint run. Formatting on `cf06` exited **0**;
explicit-path Semgrep on `cf06` exited **0**, 33 files/203 rules/zero findings.
Logs are `/tmp/task900-clippy-canonical.log`, `/tmp/task900-fmt-canonical.log`
and `/tmp/task900-semgrep-canonical.log`.

The release build at `cf06` exited **0**, then the two required emitted
executables ran once each, serially after owned builds/tests/scans stopped.
Both processes exited **0**. The 1,200-request summary/filter-choice cohort
reported maximum **98.262 ms**, largest p99 **96.666 ms**, and maximum serialized
result **340,012 bytes**. Its observed margin under 100 ms is only **1.738 ms**;
there was no rerun to obtain a better sample. The 600-request count/lookup
cohort reported maximum **1.535 ms**, largest p99 **1.430 ms**, and maximum
serialized result **7,758 bytes**. These are absolute results for this cohort,
not a causal comparison with earlier cohorts or arbitrary-load guarantees.

`/tmp/task900-release-canonical-summary.log` and
`/tmp/task900-release-canonical-query.log` record exact source/executable
commands, binary SHA-256 and process exits. The identities are respectively
`2d97de90d321ec9ad76741e32d5490d8776b37ba1bea1daa5b4866f4ca9d0678` and
`869d7d685166203b265c8eb4fa89e9d9f9065d57dbb123f0309e9b443252fdb7`.
The [performance record](task-query-performance.md) contains all 18 release
rows, sampled-byte semantics and separate startup/write/memory/storage evidence.
Read-gate success does not waive these costs; an exact-base cost comparison is
not claimed.

Final independent scope/code/Astra dispositions cover the entire expanded
repair and documentation delta and are recorded on task #900, together with
the final pre-push Gitleaks result and exact handoff head. This ledger does not
self-approve either Astra finding or authorize #891 consumer activation.

## Second withdrawn candidate: production refresh and choice round trips

Independent Sol scope and separate code inspection approved exact
`48453721493ef03af2eb26bf60124b72d0040396` against the original exact base,
without rerunning gates. Independent Astra reviewer `/root/astra_final_review`
(gpt-6-astra) then requested changes on that same head. The original pair of
canonical timestamp/assignee findings was closed by that inspection; two new
P2 findings remained. The handoff is withdrawn again. These dispositions must
not be merged into an approval of a later production descendant.

First, the actual query-fact refresh statement used an OR around the dismissal
floor, preventing an indexed meaningful-history range seek. New tests execute
the exact production INSERT, with its bindings, after forcing the eligible
legacy-attention branch. Histories of 10, 10,000 and 50,000 entries test absent,
present-empty, intermediate and high dismissal floors, plus a separate empty
history-ID suffix control. The RED run exited **101**, **1 passed, 1 failed**
(`/tmp/task900-refresh-cost-red.log`). At 50,000 entries the high-floor case
used **250,241 VM steps**, versus **245** without a floor; its actual query plan
did not seek an `ulid` range. Both physical work and selected attention results
must pass on the fixed statement; merely adding CASE syntax is insufficient.

Second, exact canonical selection and display-derived filter choices disagreed.
The actual RPC choice-to-selection RED run exited **101**, **1 passed, 3 failed**
(`/tmp/task900-choice-rpc-red.log`); Store choice regressions exited **101**,
**0 passed, 2 failed** (`/tmp/task900-choice-store-red.log`). Fixtures retain
sole NUL-bearing labels/agent IDs, collisions with ordinary prefixes,
disappearance/reassignment and stale-database repair. Pending-only tasks must
advertise the existing `any` selector, never an exact pending-target assignment
or `none`; same-task pending/assigned/unassigned transitions remain covered.

Source audit at exact base `12b8` and candidate `484` found no active remote
choice consumer applying the incompatible local filter: the shipped RPC name
occurs only in SQL-credit admission, and tracker menus derive from `state.all`.
At frozen #891 checkpoint `5a04b2658204ad794fb97910e9412d88aa0aca86`, the new
remote reader has no production importer. This resolves the identified
base-scoped exposure, not future activation. Current local `any`/`none` behavior
still differs for pending-only tasks. #891's rendered-choice, dispatch, display
and dual current-consumer/canonical review gate is separately recorded at
`#891/c/tc-01M64A9D058M2B7EMP3N30G6YM`; backend RPC tests do not satisfy it.

The fixed focused runs exited **0**: five actual RPC cases, two Store choice
cases, two actual production-statement cost cases and the existing invalid-choice
guard. Final logs are `/tmp/task900-choice-key-bound-green.log`,
`/tmp/task900-choice-store-final-green.log`,
`/tmp/task900-refresh-cost-final-green.log` and
`/tmp/task900-choice-invalid-final-green.log`. The same debug test executable
has SHA-256 `c14e6d0ff43155944998389818d708895f6cb7ed09128cb0bb7b72e071d44293`;
its source is committed as `fa8a4ff9b` (with new tests first compiled in the
working tree). The actual statement uses **247–250 VM steps** for all twelve
history/floor combinations; EXPLAIN shows the covering meaningful index and
`task_id=? AND ulid>?` in the present-floor branch. The null/empty result
distinction is separately asserted.

A further RED run (`/tmp/task900-choice-key-bound-red.log`) exited **101**:
a valid 128-byte NUL-bearing agent ID exceeded the old 512-byte escaped choice
key bound. The repair keeps raw IDs at 128 UTF-8 bytes, expands only assignee
keys and cursor positions to 1,024 bytes, and retains value/response/envelope
limits. The executable three-page, limit-one RPC test compares exact selections
for 127-NUL and 128-NUL prefix-colliding IDs and an ordinary ID. Full encoded
cursors, after JSON escaping and base64, measure **1,303** and **1,312 bytes**,
below the unchanged 2,048-byte envelope cap; their inner keys are 792/798 bytes.

Fresh production-descendant full gates, matched cost evidence and independent
scope/code/Astra review remain pending. Earlier read latency cohorts retain
their named source identities; no sample is rerun to improve its observed margin.

## Range/choice repair: full suite and test-only lint follow-up

The clean committed-head full run at
`34dc4db3a1e6046fbf00a1991772a8d666a4d901` exited **0**: **6,004** unit tests
passed, 24 ignored, **56** API contracts passed, all normal integration suites
and doc tests passed. `/tmp/task900-cargo-range-choice-full.log` records the
source head, exact `nice -n 10 cargo test -- --nocapture` command and final
`process_exit=0`. This includes the production range/choice repair and the
expanded cursor/lifecycle tests; the earlier `cf06` full run remains separate.

The subsequent `nice -n 10 cargo clippy --all-targets -- -D warnings` at the
same `34dc` head exited **101**. The sole finding is cognitive complexity 22/15
in the pending-choice lifecycle test, not a production failure. The original
log `/tmp/task900-clippy-range-choice.log` remains retained. The sequential
gate wrapper stopped there, so no release build or measurement ran in that
invocation. The test-only follow-up extracts helpers while retaining all
same-task transitions, cancellation, reassignment and repair assertions;
no suppression or ratchet edit is authorized. The 34dc full-suite result must
not be relabeled as covering that extraction descendant.

The test-only helper extraction is committed as `f85b2b803`. The affected five
RPC tests exited **0** (`/tmp/task900-choice-test-refactor-green.log`); the
emitted debug test executable SHA-256 is
`ced8bff74287f3a643f4ada8545ea81f0a0d380f721847be8b7bad79a6b530f4`.
The delta changes one test file, retaining every transition assertion and
production source. Clippy, scans and release qualification must run on the
descendant; no additional full suite is requested solely for bookkeeping.

## Final repaired production and test-only descendant gates

At clean committed `55784b050e7294b68830bc3c82beeb2a72c86b31`, Clippy with all
targets and warnings denied exited **0**; formatting exited **0**; explicit-path
Semgrep exited **0**, 39 tracked files, 445 rules, zero findings. The logs are
`/tmp/task900-clippy-choice-extraction.log`,
`/tmp/task900-fmt-choice-extraction.log` and
`/tmp/task900-semgrep-choice-extraction.log`, each recording source, command
and final process exit. The full Cargo run remains attributed to `34dc`, and
the five affected RPC tests to the test-only `f85` extraction. No production
change follows `fa8a4ff9b`; later committed changes are test helpers and evidence.

The three-target release build at `55784b050` exited **0**. Each emitted read
gate then ran **once**, serially with other owned builds/tests/scans stopped,
and exited **0**. Summary/choice maximum **94.649 ms**, largest p99 **94.247 ms**,
maximum sampled serialized result **340,012 bytes**. Aggregate/lookup maximum
**3.300 ms**, largest p99 **1.973 ms**, maximum result **7,758 bytes**. Lookup's
own maximum is 3.300 ms at 100 tasks (5,000-task lookup maximum 0.711 ms);
the old `cf06` lookup maximum 0.554 ms is historical, not this repaired run.
The old `cf06` summary maximum 98.262 ms and 1.738 ms margin remain retained;
the new samples are production-descendant qualification, not attempts to
improve a passing cohort. These results do not establish causal differences.

Build and raw run logs are `/tmp/task900-release-choice-extraction-build.log`,
`/tmp/task900-release-repaired-summary.log` and
`/tmp/task900-release-repaired-query.log`. Commands, binary hashes, all 18
scenario rows and sampled-byte semantics are in the
[performance record](task-query-performance.md).

The matched exact-base cost cohort ran only after both read gates stopped.
All **24** direct processes, the guarded runner and parser exited **0**, with
**552** raw operation samples. The [cost record](task-query-cost/README.md)
retains source/binary/fixture identity, order, raw samples, CPU and raw HWM.
It shows added repair/reopen/write costs; cost acceptance remains unresolved
and is separate from correctness and scope review. HWM is not allocator/RSS
delta, and matched RSS/storage/empty-startup evidence is absent. No #890 waiver
or no-regression claim follows from these passes.

The first diff-format check of generated cost CSV artifacts exited **2** for
CRLF line endings. Commit `7ad0d8117` normalizes them to LF without changing
samples; subsequent staged/range format checks exited **0**. This artifact
format failure is retained in the cost record separately from the successful
measurement processes. The final compact record includes all 20 wall/CPU
median-and-range rows, exact outer commands and inherited-priority limits.

The final independent scope/code dispositions must inspect the test-helper and
documentation delta in addition to their full `34dc` repair inspection. Fresh
independent Astra disposition on the complete final head must close the two
new range/choice findings; original `0d3` findings remain separately recorded as
closed at the `484` inspection. Final review dispositions and pre-push scan are
recorded on #900 rather than inferred from this implementation ledger.


## Failed serial gate and descendant validation

The coordinator serial gate `e4a105b5125b01a96030233a91f18e25bfb4cc02` failed:
Cargo exited **101** (6,003 passed, 1 failed, 24 ignored; primary RunFailed count
expected 2/observed 1, followed by a secondary PoisonError), and SPA exited
**1** (11,973 passed, 1 failed; sender SQL-read method parity omitted counts and
lookup). Other attachment-reported gate components exited 0. The serial attempt
from reviewed `777322d0f` against unchanged dev `12b8f869` was not pushed. Its
old reviews and accepted cost evidence remain attributed to that head; this
failed user gate is not relabeled as passing.

[The serial-gate record](task-query-serial-gate.md) preserves the original gate,
SQL credit repair `228d921e3`, and identical-overlay controlled runtime proof on
base and candidate. Both controlled runs exited **101** only at the old immediate
expected-2/observed-1 assertion after cleanup, without poisoning. They show the
same admissible model/pump-before-ticket-settlement ordering, not the unique
schedule or incidence of the original gate failure and not an explanation for
an earlier 209 ms `agent.deliver` frame. The semantic test synchronization repair and
cfg(test)-only seam are committed as `3a04e3926072b0b756f43c43923e2ffe70bb928a`.
Their whole seven-case `adk_turn_failures` focused run exited 0 in 60.90 seconds;
its binary SHA-256 is `d5eaa40e8f1f22b7dc0deb836ac5203898fe6f904ab4ad82268cdc3af5152237`.

The validation map now explicitly covers bridge `SQL_READ_METHODS` → SPA sender
`SQL_READ_METHODS` parity **and** actual mixed eight-credit admission/release for
counts/lookup at interactive/background priority across carriers. This shared
dispatcher gate applies even while the new bounded consumers are held. Source
commit `228d921e3` has observed RED baseline 1/25 and expanded 4/27 failures,
then **85/85 focused SPA cases passing**, plus focused ESLint/Semgrep exit 0;
commands, source boundaries and logs are in the serial-gate record. This is an
integration finding in a category also encountered in #890, not a reason to
waive the cross-tier gate.

The first planned focused compile at
`/tmp/task900-managed-semantic-sync-green.log` was cancelled with actual exit
**130 before test execution** after independent review caught a moved
post-coordinator-window input assertion; it is not a pass. The repaired source
keeps strict failure/comment/input assertions both before and after the
original coordinator-two-notice wait. The fresh whole `adk_turn_failures`
focused run at `/tmp/task900-managed-semantic-sync-final-green.log` exited
**0**, **7 passed**. Full Cargo/lint/format/scans and fresh independent
exact-head scope/code/Astra dispositions are required for the frozen final
candidate; their source SHA, commands and actual exits are recorded on #900.
Affected SPA suites are the requested SPA scope;
no complete-SPA rerun is implied. No new read-latency run is required for a
confirmed cfg(test)-only runtime seam/test repair under the lead's stated
boundary. Existing consumer activation holds and exact backend semantics remain
unchanged.


Zech's scoped cost acceptance for exact `777322d0f` is preserved at
#887/c/tc-01M64M7EMJ8JEB717BMD9SB3SC, resolving
#887/c/tc-01M64CAH9X97A98FYEJAZCAS1V. Its measurement limits and added costs
remain documented. The serial repair changes client admission and test-only
Rust synchronization, with no change to non-test Rust query/Store behavior;
this source boundary explains retaining the existing read-latency evidence,
not a new timing or blanket acceptance claim. Final reviewed-descendant
readiness and serial integration remain separate from that scoped decision.
