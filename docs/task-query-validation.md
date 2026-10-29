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

Final full Cargo, Clippy, formatting, scans, isolated release timing and the
final exact-head scope/code/Astra dispositions will be recorded after they
finish. None of the earlier evidence approves a later head.
