# Task query performance evidence

The comparison base for #900 is `12b8f869bd94e259666df69f1a8c0cde5c695982`. Counts, point lookup, ordering and
exact membership maintenance extend its existing materialized task summaries.
Search stays outside this conversion. A successful bounded read gate does not
waive startup, storage or write costs.

## Reproduction and boundaries

From `bridge/`, build before measuring, then run serially:

```sh
nice -n 10 cargo test --features testing --test bounded_task_query_perf -- --nocapture --test-threads=1
nice -n 10 cargo test --release --features testing --test bounded_task_query_perf --no-run
nice -n 10 cargo test --release --features testing --test bounded_task_query_perf aggregate_lookup_carrier_release_latency_gate -- --ignored --exact --nocapture --test-threads=1
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

## Evidence status

Candidate execution results and exact process exit codes are pending. No
latency pass or exact-base no-regression result is claimed by the fixture alone.

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
