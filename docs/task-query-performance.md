# Task query performance evidence

The comparison base for #900 is `12b8f869bd94e259666df69f1a8c0cde5c695982`. Counts, point lookup, ordering and
exact membership maintenance extend its existing materialized task summaries.
Search stays outside this conversion. A successful bounded read gate does not
waive startup, storage or write costs.

## Reproduction and boundaries

From `bridge/`, build before measuring, then run serially:

```sh
nice -n 10 cargo test --features testing --test bounded_task_query_perf -- --nocapture --test-threads=1
nice -n 10 cargo test --release --features testing --test bounded_task_query_perf --test bounded_task_carrier_perf --no-run
# Run the emitted release executables serially, with other gates/builds stopped:
nice -n 10 <bounded_task_carrier_perf-executable> bounded_reads_carrier_max_and_p99_with_both_locks_held --ignored --exact --nocapture --test-threads=1
nice -n 10 <bounded_task_query_perf-executable> aggregate_lookup_carrier_release_latency_gate --ignored --exact --nocapture --test-threads=1
```

The default test checks actual responses and their byte bounds without a latency
threshold. The ignored timing test refuses debug builds and asserts that **every
sample** is at most 100 ms. Each mode issues 100 serial encrypted carrier requests
for counts with 100 explicit agent buckets and 100 for an existing number
lookup, at each of 1, 100 and 5,000 tasks (600 requests). The actual app mutex,
Store writer mutex and independent SQLite `BEGIN IMMEDIATE` lock remain held
throughout each fixture's reads. This exercises carrier admission and the SQL
lane rather than calling an aggregate helper directly. Assertions cover exact
project and agent counts, lookup identity/title/number, absence of body, basis,
and the tighter 65,536-byte point-result bound (also below the 1 MiB carrier cap).
Tasks are assigned evenly across `agent-0` through `agent-99`; bucket `i` must
contain `(task_count + 99 - i) / 100` unfinished tasks. The larger fixtures
populate every requested bucket. Assignees have no roster rows, so lookup also
exercises the bounded unknown-identity fallback and verifies that one identity
is returned. This does not measure rich roster metadata or live overlays.
Reported p99 is nearest rank (sample 99 of 100); max enforces the gate.

All roots, config, repository, store, HarnessContext socket and provider PATH
are private temporary fixtures. These tests do not launch the daemon or access
the user's store, home configuration, live bridge or network. The existing eight
synchronously initialized readers and the original write/carrier benchmarks
remain unchanged. Setup/seeding is outside request timers.

A separate default test reports empty startup, forced definition repair/backfill
of 100 small assigned tasks, and three completed reopens. This measures total
forced repair, including existing summary work; it does not isolate #900's
incremental migration cost. Each reopen performs three rounds of
ordinary metadata write, history append, read-marker write and workspace/tracker
relation write. Canonical field assertions accompany measurements. Thread CPU
and wall times are separate. RSS/HWM are raw process values, including fixture
setup; storage is the sum of database, WAL and SHM file lengths, including free
pages and current log allocation. Neither is an incremental allocator or logical
payload measurement. First-after-reopen flags apply to the first metadata write;
other first-operation kinds run after that metadata write.

## Recorded debug evidence

The completed full-suite log `/tmp/task900-cargo-final-fixed.log` records source
`622f449bffadef4b2f3046b61288c53d2a307ff9`; the parent recorded process exit **0**.
The library group passed 5,978 tests with 24 ignored. The query performance
binary passed both default tests with its release timing test ignored. The
inherited write performance binary ran its six default tests. These are
**absolute debug-profile observations from an integrated correctness run**;
within-binary tests ran concurrently, so neither their wall times nor raw
process memory values are isolated latency or incremental-memory estimates.
The descendant `2c2b3f31fbcd797681747af6f0f16e603085455d` extracts test helpers
without changing production behavior; the rows below remain attributed to
`622f449b`, rather than relabeled as executions of that descendant.

New query cost fixture (milliseconds; medians with observed ranges where n > 1):

| Operation | n | Wall ms | Thread CPU ms |
| --- | ---: | ---: | ---: |
| Empty startup | 1 | 85.467 | 82.740 |
| Seed 100 assigned tasks | 1 | 249.591 | 246.962 |
| Forced definition repair/backfill, 100 tasks | 1 | 178.177 | 176.597 |
| Completed reopen | 3 | 15.191 [15.140–15.780] | 15.071 [15.016–15.598] |
| Ordinary metadata write | 9 | 2.375 [2.325–5.298] | 2.354 [2.302–5.245] |
| History append | 9 | 2.187 [2.117–3.348] | 2.172 [2.104–3.325] |
| Read-marker write | 9 | 2.481 [2.411–4.306] | 2.451 [2.393–4.253] |
| Workspace/tracker relation write | 9 | 2.330 [2.290–2.795] | 2.315 [2.276–2.712] |

The three first-after-reopen metadata writes were 5.298, 4.503 and 4.526 ms.
Raw reported RSS/HWM was 51,300 KiB after empty startup, 75,448 KiB after repair,
and 75,480 KiB at the end. Reported DB+WAL+SHM lengths were 3,410,000 bytes after
repair and 3,447,080 / 3,385,280 / 3,385,280 bytes after successive write cycles.
This allocation variation is not a per-row storage-growth estimate.

The inherited unchanged write fixture supplies separate large-record/history
coverage: one task, 10,000 initial comments, a 2,900,000-byte task body and
2,259,994 bytes of comment bodies (5,159,994 combined body bytes). Serialized
canonical inputs total 7,240,464 bytes; each appended comment is 214 bytes.
The small counterpart has a 17-byte body and initially no history.

| Inherited Store operation | n per size | Small wall ms, median [range] | Huge wall ms, median [range] |
| --- | ---: | ---: | ---: |
| Initial migration/startup | 1 | 46.172 | 289.135 |
| First five appends | 5 | 1.977 [1.799–3.172] | 160.164 [156.624–171.493] |
| Completed reopen | 5 | 24.677 [16.365–30.731] | 15.655 [15.124–16.032] |
| First append after reopen | 5 | 3.780 [2.419–3.959] | 163.045 [161.732–164.306] |
| Warm append after reopen | 10 | 1.932 [1.352–3.813] | 160.519 [157.849–176.802] |
| First changed-metadata plus comment write | 1 | 6.819 | 175.039 |
| Warm changed-metadata plus comment write | 4 | 4.343 [4.076–4.966] | 156.787 [155.653–176.308] |
| First changed write after schema invalidation | 5 | 10.076 [9.482–14.268] | 164.370 [160.689–166.244] |
| Warm changed write after schema invalidation | 10 | 4.085 [3.319–5.789] | 160.785 [157.095–167.567] |

These remain public Store measurements, distinct from the inherited raw-SQL
cache-pressure diagnostics. The large-fixture initial migration consumed
286.358 ms of thread CPU; small consumed 44.799 ms. Maximum reported raw HWM in
the concurrent inherited write fixture process reached 104,172 KiB. No claim
about an isolated allocation increment follows from that maximum.

Debug carrier observations (100 samples in each row; correctness and byte checks
passed, with no 100 ms performance assertion in this mode):

| Tasks | Method | Result bytes | p99 ms | Max ms |
| ---: | --- | ---: | ---: | ---: |
| 1 | tasks.counts | 7,652 | 12.689 | 12.948 |
| 1 | tasks.lookup | 877 | 3.088 | 3.112 |
| 100 | tasks.counts | 7,656 | 12.964 | 13.788 |
| 100 | tasks.lookup | 888 | 3.037 | 3.309 |
| 5,000 | tasks.counts | 7,758 | 12.009 | 12.677 |
| 5,000 | tasks.lookup | 892 | 3.085 | 3.215 |

## Completed isolated release evidence

Both required gates passed at exact source
`2c2b3f31fbcd797681747af6f0f16e603085455d`. The build and both direct-executable
processes exited **0**, as recorded by the parent. Logs are
`/tmp/task900-release-build.log`, `/tmp/task900-release-summary.log` and
`/tmp/task900-release-query.log`. After the build, the parent stopped its other
Cargo, scan and test work, then ran these two binaries once each, serially;
there were no timing reruns. This establishes the recorded isolated cohort,
not whole-machine idle or performance under every possible external load.

Commands from `bridge/`:

```sh
nice -n 10 cargo test --release --features testing --test bounded_task_query_perf --test bounded_task_carrier_perf --no-run
nice -n 10 target/release/deps/bounded_task_carrier_perf-fe5c6190a1faf97e bounded_reads_carrier_max_and_p99_with_both_locks_held --ignored --exact --nocapture --test-threads=1
nice -n 10 target/release/deps/bounded_task_query_perf-a9b710faaf70c464 aggregate_lookup_carrier_release_latency_gate --ignored --exact --nocapture --test-threads=1
```

Binary SHA-256 identities:

| Release executable | SHA-256 |
| --- | --- |
| `bounded_task_carrier_perf-fe5c6190a1faf97e` | `03351594909ad22f06a2aa708438c6c9c10cd39e6022187add9448f23c641462` |
| `bounded_task_query_perf-a9b710faaf70c464` | `c8cb66515a570161617af2779f6f473c6a4727befdc9e8d2bbb4835d44220358` |

Each table row contains 100 serial requests. **Bytes is the maximum serialized
result JSON size across those 100 samples**, excluding the RPC response and
encryption envelopes. The 1 MiB summary/choice and 64 KiB point-result gates
therefore do not assert total encrypted frame size. Timing includes encrypted
carrier admission, receipt of the matching encrypted response and client decode;
fixture construction and seeding are excluded. Both binaries hold the actual
app mutex, Store writer mutex and independent SQLite write lock during reads.

The existing summary/filter-choice gate covers a changed path: #900 introduced
ordered fact candidate reads and predicate point seeks. All 12 scenarios
(1,200 requests) passed the 100 ms per-sample maximum and 1 MiB result bounds:

| Tasks | Scenario | Returned rows | Max result bytes | p99 ms | Max ms |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | Summary default limit 100 | 1 | 955 | 0.675 | 1.707 |
| 1 | Summary limit 500 | 1 | 955 | 0.544 | 0.571 |
| 1 | Summary absent-label filter | 0 | 283 | 0.381 | 0.397 |
| 1 | Label filter choices | 2 | 79 | 0.304 | 0.341 |
| 100 | Summary default limit 100 | 100 | 67,768 | 19.347 | 20.302 |
| 100 | Summary limit 500 | 100 | 67,768 | 17.131 | 20.524 |
| 100 | Summary absent-label filter | 0 | 285 | 3.265 | 3.271 |
| 100 | Label filter choices | 100 | 1,351 | 0.490 | 0.648 |
| 5,000 | Summary default limit 100 | 100 | 68,412 | 19.874 | 19.924 |
| 5,000 | Summary limit 500 | 500 | 340,012 | 79.367 | 81.353 |
| 5,000 | Summary absent-label filter | 0 | 513 | 17.128 | 32.618 |
| 5,000 | Label filter choices | 100 | 1,351 | 0.499 | 0.504 |

The new aggregate/lookup gate passed all six scenarios (600 requests), including
populated agent buckets and lookup identity fallback, with every result below
65,536 bytes and every sample below 100 ms:

| Tasks | Method | Max result bytes | p99 ms | Max ms |
| ---: | --- | ---: | ---: | ---: |
| 1 | tasks.counts | 7,652 | 1.304 | 1.596 |
| 1 | tasks.lookup | 877 | 0.425 | 0.484 |
| 100 | tasks.counts | 7,656 | 1.628 | 1.629 |
| 100 | tasks.lookup | 888 | 0.439 | 0.468 |
| 5,000 | tasks.counts | 7,758 | 1.670 | 1.861 |
| 5,000 | tasks.lookup | 892 | 0.430 | 0.677 |

These are separate gates over their stated fixtures. Their pass does not waive
startup or write costs, establish exact-base no-regression, or prove a universal
latency bound for arbitrary imported records, selector distributions or load.

## Comparison limits

The unchanged `bounded_task_write_perf.rs` remains the appropriate common
fixture for base/candidate comparisons of small and 10,000-comment workloads,
including repeated post-reopen and schema-invalidated writes. Historical #890
reports comparing `c6b1ae4f` with `861192c0` describe earlier revisions, not the
#900 exact base `12b8f869b`. Their timings must not be relabeled as measurements
of this base. No new checkout or clone was created for this extension.

The extra query fact, membership and aggregate updates happen in supported
canonical writer transactions. They add real startup/backfill work, retained
storage and per-write cost; carrier success alone cannot establish those costs
are unchanged. Imported canonical row byte size remains unbounded during
backfill; batch cardinality does not bound decoding one arbitrarily large row.
