# Matched #900 Store cost cohort

All **24 direct processes exited 0**; runner and parser also exited 0. The
measurements show increased candidate costs. This is evidence for the lead's
explicit disposition, **not cost acceptance or a no-regression pass**.

The approved native Build-managed baseline workspace was detached at exact
`12b8f869bd94e259666df69f1a8c0cde5c695982`. Candidate source was exact
`55784b050e7294b68830bc3c82beeb2a72c86b31` (production repair `fa8a4ff9b`, with
subsequent test-helper/docs changes). Both sources stayed clean during sampling.
No manual checkout, clone or fixture-source copy was created. The existing
`bridge/tests/bounded_task_write_perf.rs` was byte-identical at both sources:
SHA-256 `fb478ebb1b143cc455fbe7f93fa28d72559a3b5e26503dce75f8c8337ee1e92d`.

## Protocol and provenance

Both binaries were built before any samples, in the same release profile
(opt-level 3, debug assertions disabled) with feature `testing`. The shared Cargo
target was reused for compilation, but immutable mode-0555 executable copies
were preserved outside it before the other build could replace the same Cargo
filename. Baseline SHA-256 is
`6550036e7792634905d44f892f3d753a5c961b699bb39271bd84256bba69200c`;
candidate SHA-256 is
`1b05323973754d2e3adea668fd7c4654c76d2642baf7fdd840bd4037147f3e62`.
Builds each exited 0; logs/paths/profile/API guards are in [manifests.json](manifests.json).

Baseline build, from its `bridge/` directory:

```sh
CARGO_TARGET_DIR=/home/zech/Projects/.build-worktrees/workspaces/proj-1/task-summary-f6a425fe-a96d-4e2d-8f98-50a767de5498/Build/bridge/target nice -n 10 cargo test --release --features testing --test bounded_task_write_perf --no-run --message-format=json
```

Candidate build, from its `bridge/` directory:

```sh
nice -n 10 cargo test --release --features testing --test bounded_task_write_perf --test bounded_task_query_perf --test bounded_task_carrier_perf --no-run
```

Only the four public Store fixtures below were measured; both raw-SQL
cache-pressure tests were excluded. Three rounds ran every fixture with adjacent
base/candidate processes in this exact order: round 1 base→candidate, round 2
candidate→base, round 3 base→candidate. Each process used a fresh temporary store.
There were no timing retries. Builds, scans, other tests and carrier measurements
had stopped before this cohort, and none ran alongside it in this task. No
whole-machine idle claim follows from that coordination.

Exact execution and parsing commands, from the candidate repository root:

```sh
python3 docs/task-query-cost/compare.py run --base /tmp/task900-cost-baseline-manifest.json --candidate /tmp/task900-cost-candidate-manifest.json --output /tmp/task900-cost-cohort-55784b05
python3 docs/task-query-cost/compare.py parse --output /tmp/task900-cost-cohort-55784b05
```

Each direct command was `nice -n 10 /tmp/task900-cost-{baseline|candidate}-bin
<exact-test-name> --exact --nocapture --test-threads=1`, with
`TASK890_EXPECTED_API=3.26.0` / `3.27.0` respectively. The optional SQL profiler
was absent. Every run rechecked source HEAD/cleanliness, fixture SHA and binary
SHA before and after execution. [runs.json](runs.json) retains all 24 commands,
actual exits, source/binary/fixture identities, ordering and elapsed process
observations. Process elapsed time is not substituted for the instrumented
Store operation times below.

The runner inherited niceness **10** for every process; `nice -n 10` requests
another increment of 10. Linux's clamp implies effective child niceness 19;
actual per-child niceness was not directly sampled. Both variants used the same
requested command and inherited priority. This is not an explanation or waiver
for their differing costs. No OS cache flush occurred: new process is not a
claim of disk-cold state.

## Scope of the common fixture

`identical_small_task_write_fixture` and
`identical_huge_history_write_fixture` remove summary/filter projections and
checkpoints, set the old schema version, then measure public `Store::new`.
This is total **forced projection repair/startup**, including schema, retained
projection work and reader admission, rather than isolated #900 migration cost.
They then record five initial appends and five completed reopens, each followed
by one first and two warm appends. Small uses one 17-byte-body task and no
initial history. Huge uses one 2,900,000-byte-body task and 10,000 comments with
2,259,994 combined body bytes; serialized canonical inputs total 7,240,464
bytes. Each added comment is 214 bytes.

`identical_small_metadata_write_fixture` and
`identical_huge_metadata_write_fixture` change title, labels and updated time
and append a comment on every write. They record one first/four warm writes,
then five deliberate schema invalidations, each followed by one first/two warm
writes. Their setup includes fixture initialization/repair but does not isolate
repair in a separate startup timer; it is not pooled into the startup rows.

The common fixture reports raw `VmHWM` snapshots, **not RSS or retained storage**.
No new measurement harness was added to fill those gaps. There is also **no
matched empty-store startup** measurement. Historical unmatched RSS/storage or
empty-startup values cannot fill these holes. The fixture does not cover every
possible producer, relation/read-marker workload or selector distribution.

## Measured Store operation costs

Values below are wall milliseconds: median [minimum–maximum]. Counts are per
variant; separate CPU medians/ranges and every unrounded sample are retained in
[summary.json](summary.json) and [samples.csv](samples.csv). Within-process writes
share state, so they are not independent process replicates. The three rounds
are a small cohort, without confidence intervals or a universal latency claim.

| Workload | Phase | n | Base wall ms | Candidate wall ms |
| --- | --- | ---: | ---: | ---: |
| Small append | forced repair startup | 3 | 10.1170 [7.4620–10.5530] | 11.7080 [11.5790–11.9560] |
| Small append | initial first | 3 | 0.4930 [0.4810–0.5390] | 0.7500 [0.7490–0.7700] |
| Small append | initial warm | 12 | 0.1720 [0.1620–0.2300] | 0.3825 [0.3660–0.3950] |
| Small append | completed reopen | 15 | 4.7350 [4.5980–4.9250] | 5.4660 [5.3750–7.4070] |
| Small append | post reopen first | 15 | 0.5560 [0.5300–0.6100] | 0.9410 [0.9230–1.0670] |
| Small append | post reopen warm | 30 | 0.1755 [0.1700–0.2540] | 0.3840 [0.3640–0.4370] |
| Huge append | forced repair startup | 3 | 91.6970 [88.5470–94.1380] | 107.8860 [103.1840–108.0020] |
| Huge append | initial first | 3 | 15.3510 [15.0360–16.6060] | 16.5960 [16.1310–16.6280] |
| Huge append | initial warm | 12 | 11.4955 [11.2330–12.1890] | 11.8960 [11.5220–13.1340] |
| Huge append | completed reopen | 15 | 4.8430 [4.7990–5.2200] | 5.5730 [5.4580–6.0370] |
| Huge append | post reopen first | 15 | 10.7300 [10.2790–13.0870] | 11.5200 [11.0620–12.2550] |
| Huge append | post reopen warm | 30 | 10.4545 [10.2290–12.7410] | 11.0760 [10.6630–12.4180] |
| Small metadata + comment | metadata first | 3 | 0.7320 [0.7110–0.8120] | 1.3220 [1.2230–1.4200] |
| Small metadata + comment | metadata warm | 12 | 0.3810 [0.3530–0.4670] | 0.8955 [0.8420–0.9840] |
| Small metadata + comment | metadata invalidated first | 15 | 2.0630 [1.9980–2.5090] | 2.9150 [2.8230–3.4260] |
| Small metadata + comment | metadata invalidated warm | 30 | 0.3800 [0.3510–0.4390] | 0.8770 [0.8410–0.9840] |
| Huge metadata + comment | metadata first | 3 | 21.2880 [20.2220–21.3890] | 23.4190 [22.9120–23.4790] |
| Huge metadata + comment | metadata warm | 12 | 12.3035 [11.6980–12.8390] | 12.7830 [12.3970–12.9650] |
| Huge metadata + comment | metadata invalidated first | 15 | 14.0400 [13.8050–15.0170] | 14.9970 [14.7480–15.8160] |
| Huge metadata + comment | metadata invalidated warm | 30 | 12.1200 [11.8360–13.2820] | 12.7460 [12.4740–13.3950] |

The candidate increases observed in this cohort include small warm appends
0.1720→0.3825 ms, small warm metadata/comment writes 0.3810→0.8955 ms,
small completed reopens 4.7350→5.4660 ms, and huge forced repair/startup
91.6970→107.8860 ms. These increases are retained for disposition; successful
execution does not convert them into an accepted exception. The comparison is
between the exact sources above, rather than an earlier historical #890 report.
It does not isolate the cause of each delta among the candidate's changes.

## Raw process memory observations

The next table lists each process's maximum **reported** `VmHWM` snapshot in
round order, in KiB, followed by the median. Values include fixture setup,
libraries, allocator state and test execution. They are not isolated allocation
increments; repeated operation snapshots do not constitute independent memory
samples. All operation-level raw snapshots remain in [samples.csv](samples.csv),
with per-process values in [process-hwm.csv](process-hwm.csv).

| Workload | Base round maxima KiB; median | Candidate round maxima KiB; median |
| --- | ---: | ---: |
| Small append | 9632/9624/9516; 9624 | 10028/10036/10052; 10036 |
| Huge append | 40444/40020/40216; 40216 | 40484/40308/40240; 40308 |
| Small metadata + comment | 9592/9480/9500; 9500 | 9804/9876/9948; 9876 |
| Huge metadata + comment | 34544/34480/34432; 34480 | 34828/34736/34708; 34736 |

## Retained evidence

- [record.json](record.json): cohort exits, limits, runner identity and raw-log hash.
- [manifests.json](manifests.json): both exact sources, binary/fixture identities and build provenance.
- [runs.json](runs.json): mandatory exact order, commands and all 24 process exits.
- [samples.csv](samples.csv): 552 operation samples with wall/CPU microseconds and raw HWM.
- [summary.json](summary.json): all grouped wall and CPU medians/ranges/counts.
- [process-hwm.csv](process-hwm.csv): 24 per-process raw-memory observations.
- [compare.py](compare.py): the exact guarded runner/parser used.

Full stdout/stderr plus run metadata and process exits are aggregated at
`/tmp/task900-cost-cohort.log` for the Build evidence attachment. Individual raw
logs remain at `/tmp/task900-cost-cohort-55784b05/`. The aggregate SHA-256 is
recorded in `record.json`. Correctness review and cost acceptance are separate;
the lead must explicitly disposition these results and their uncovered costs.
