# #894 append publication cost

Coalescing reduces deterministic writer work: **44 → 37 changed rows,
181 → 168 statement executions, and 127 → 122 SELECT starts** for one append.
The exact table map and complete statement map are identical at starting
histories 1, 60 and 4096 within each arm. The paired wall-clock cohort shows
**no measurable change at its noise level**. It does not establish an elapsed-
time improvement or regression; that is the lead's disposition of this cohort.

## Matched scope

The coalescing-only baseline restores exactly these three producer programs
from `e1c4b21fc3c16e1ab3a2dd70b155efff8a214c03`:

- `bridge/src/store/sync/journal.rs`
- `bridge/src/store/sync/device.rs`
- `bridge/src/store/sync/retention.rs`

Both arms use the common current Store/schema, prepared-statement cache capacity
128, synthetic append fixture and observer. Original baseline expectations are
44 rows (`sync_stream_state` ×4 and `sync_state` ×5); the coalesced program
writes 37 (`sync_stream_state` ×1 and `sync_state` ×1). Publication remains in
the canonical transaction. This isolates the coalescer; it is **not** a
comparison of entire old/new bridge binaries or schema versions.

The renamed regression,
`store::tests::conversations::appending_one_message_writes_a_constant_row_budget_however_long_the_conversation_is`,
attributes every changed row by table and checks equality across retained
history lengths. The same transaction-local scope measures executed statements
on the sole writer connection; fixture construction and verification reads are
outside the observation. These are executions, not preparation counts, VM
steps, filesystem writes or all-process SQL.

## Paired warm wall cohort

The binaries were built before this retained cohort and copied to distinct
immutable executable paths. [`append-paired/runs.json`](append-paired/runs.json)
records every exact command, binary SHA, ordering, raw log and actual exit.
All six direct processes exited **0**. Adjacent pair order was baseline →
candidate, candidate → baseline, then baseline → candidate. There were three
untimed warm-ups followed by 29 timed appends per history in each process:
87 observations per arm/history, 522 observations overall. History grows
identically through the warm sample sequence in both arms.

Every round's median is shown; the combined figure is the median of those
three process medians, not a selected best run or a pooled significance test.

| Starting history | Samples per arm | Baseline round medians µs (1 / 2 / 3) | Candidate round medians µs (1 / 2 / 3) | Median of medians µs |
| ---: | ---: | --- | --- | ---: |
| 1 | 87 | 6,729 / 4,675 / 4,508 | 4,508 / 3,877 / 6,013 | 4,675 → 4,508 |
| 60 | 87 | 5,504 / 5,562 / 4,926 | 4,974 / 6,726 / 6,396 | 5,504 → 6,396 |
| 4096 | 87 | 29,795 / 26,555 / 27,986 | 27,807 / 25,931 / 29,530 | 27,986 → 27,807 |

All six raw logs, the original summary/comparison, binary identities and
ordering are preserved under [`append-paired/`](append-paired/manifest.json).
[`samples.json`](append-paired/samples.json) retains every timed observation.
The documentation import validated all sample indices, per-round medians,
sample counts, row budgets and six recorded exits; that parser exited 0.
The mixed medians and process-to-process spread are why the wall-clock
interpretation remains “no measurable change,” despite fewer writes.

## Executed SQL and row budgets

The separate statement-count baseline used the common current test fixture,
Store/schema and 128-entry cache, with only the three coalescer paths restored
to exact `e1c4b21f` and the two original row-budget expectations reinstated.
The baseline process exited 0; each source was restored from a saved copy,
touched and byte-compared with actual exit 0. The candidate observations come
from the retained `store-tests-counted-final` gate, whose actual process exit
was 0. Later final-head qualification is recorded separately on #894.

| Starting history | Executions | SELECT starts | Changed rows |
| ---: | ---: | ---: | ---: |
| 1 | 181 → 168 | 127 → 122 | 44 → 37 |
| 60 | 181 → 168 | 127 → 122 | 44 → 37 |
| 4096 | 181 → 168 | 127 → 122 | 44 → 37 |

The comparator command, run from the repository root, exited **0**:

```sh
/usr/bin/python3 docs/task-query-cost/append-statements.py --baseline /home/zech/Projects/.agent-scratch/894/append-statements-base/baseline.log --candidate /home/zech/Projects/.agent-scratch/894/store-tests-counted-final.log --output docs/task-query-cost/append-statements
```

It requires exactly the three history lengths, verifies that each full SQL
map sums to its execution total, independently recomputes SELECT starts,
checks equality of complete SQL/table maps within each arm, and refuses an
increase in candidate executions, SELECTs or changed rows. All complete maps
are retained in
[`append-statements/comparison.json`](append-statements/comparison.json),
with raw baseline and candidate observations and fixture/log hashes in
[`provenance.json`](append-statements/provenance.json). The statement-cache
capacity-32 negative control is a separate compilation/reuse check; these
execution counts do not substitute for its failed-control/restoration evidence.
