# #894 cutover harness

`spa/test/syncTimingCutover.test.js` extends #869's logical-RPC simulation with
the real `cacheSync`, scoped runtime, scoped recovery engine, device publication
cache and `taskFeed`. Only peer responses, greetings, subscription transport and
browser-lock admission are fixtures. Cache transactions, notifications,
projections and feed rereads run against real `fake-indexeddb` event-loop work.

The Inbox is open and no conversation, checkout, task page or file is mounted.
Consequently the measured history, Git, tree, patch and diff calls are hidden
warming. The separate task-open and directory-page contract tests measure those
view reads; this startup cohort does not substitute for them.

## Matched scenarios

| Profile | Projects | Workspaces | Agents per workspace |
| --- | ---: | ---: | ---: |
| small | 2 | 2 | 1 |
| single-project | 1 | 24 | 3 |
| large | 8 | 80 | 3 |

Every profile runs once against synthetic wire 3.33 and 3.34 peers. The 3.33
peer advertises the incomplete device capability, which the current support
predicate correctly refuses. The 3.34 peer owns device summaries. Project/task
capabilities are present but no unopened scope is registered.

Each process/test creates a fresh database, then measures three successive
starts with the same workload:

1. **Cold:** scoped snapshot pages of 100 records plus the finite catch-up;
   or the legacy list pass for wire 3.33.
2. **Warm reconnect:** retain the engine/cache, replace the session/greeting,
   and publish one newer conversation summary.
3. **Long absent:** advance fixture time eight days and return
   `resync_required/retention_expired` for the prior device cursor. The real
   engine resets and replaces the device snapshot before catch-up. The absence
   interval is excluded from measured logical recovery time.

The fake board also carries full runtime run bodies in both modes, as the
production compatibility path does. They remain part of response-byte counts;
removing warmers does not claim to remove this board cost.

## Counters and limits

- Request and response bytes count serialized JSON method/params and response
  payloads, excluding encryption, wire framing and subscription setup.
- RPC count and logical time use fixed service costs, inherited from #869's
  modeling approach. For example `thread.page` costs 500 logical ms and
  `git.status` 600; no actual network delay is inserted.
- IDB callbacks count actual fake-IDB scheduled operations; cursor continuations
  count actual IndexedDB cursor advances. Cache notifications count committed
  cache announcements. These are distinct from Rust/SQLite physical row writes.
- Cache wall time is elapsed Node work through the logical-clock driver and
  fake-IDB drain. It includes JavaScript/cache/feed work and is retained as an
  observation, not a production disk or latency estimate.
- Row publication is the interval between the modeled peer summary landing and
  the first real `taskFeed` snapshot containing that summary. It records logical
  time, IDB callbacks and real wall time. This is data publication, not DOM paint.

No live bridge, installed binary, browser, server contention, encrypted
transport or production storage is exercised. Task paging, directory paging,
SQLite publication writes and producer/recovery correctness have separate
evidence; this harness cannot establish those properties.

## Baseline provenance

The matched baseline uses production SPA source at exact
`e1c4b21fc3c16e1ab3a2dd70b155efff8a214c03`, with the existing warmers in place.
`git diff --name-only spa/src` was empty when the baseline completed.
The new harness's SHA-256 was
`134883eef47c710e0a72860e96dc4cf28cf79105383f2f9c81d643c95d74c059`.
The successful direct command, from `spa/`, was:

```sh
PATH=/home/zech/.local/share/mise/installs/node/22.23.4/bin:$PATH CUTOVER_PROFILE_OUTPUT=/home/zech/Projects/.agent-scratch/894/cutover-before-v2.jsonl CARGO_BUILD_JOBS=8 nice -n 10 node_modules/.bin/vitest run test/syncTimingCutover.test.js > /home/zech/Projects/.agent-scratch/894/cutover-before-v2.log 2>&1
```

It exited **0**: six tests, all 18 profile/start rows. Raw retained records are
[`cutover-before.jsonl`](cutover-before.jsonl).

An earlier fixture attempt exited **1** in the scoped single-project and large
warm scenarios: it changed an agent's summary without advancing its activity
timestamp, so a different agent remained the latest summary and no row witness
could match. The fixture was corrected to advance that timestamp; the changed
fixture also witnesses legacy board-to-feed publication. The earlier output
remains in the task's scratch logs (`cutover-before.log` and JSONL) and is not
used as the matched baseline. No unchanged test retry was used.

## Comparison

Run the identical harness after implementation with a distinct output path
and `CUTOVER_EXPECT_LAZY=1`. The latter additionally asserts that an Inbox-only
scoped start issues no thread, Git, Files or task detail/list warmers. Keep the
source/harness identities and direct exit codes with the cohort.

```sh
python3 docs/task-query-cost/cutover.py docs/task-query-cost/cutover-before.jsonl docs/task-query-cost/cutover-after.jsonl --output docs/task-query-cost/cutover-comparison
```

The parser refuses mismatched/duplicate profile rows or absent row-publication
witnesses. It emits CSV plus a before/after Markdown table. Small-profile
response bytes, RPCs, logical time and actual cache callback counts must not
increase. Wall times remain explicit observations and require interpretation
with their measurement limits. Baseline-vs-itself parser control exited 0 with
18 rows and no reported small-account regressions.

## Candidate cohort

The candidate direct command, from `spa/`, was:

```sh
PATH=/home/zech/.local/share/mise/installs/node/22.23.4/bin:$PATH CUTOVER_PROFILE_OUTPUT=/home/zech/Projects/.agent-scratch/894/cutover-after.jsonl CUTOVER_EXPECT_LAZY=1 CARGO_BUILD_JOBS=8 nice -n 10 node_modules/.bin/vitest run test/syncTimingCutover.test.js > /home/zech/Projects/.agent-scratch/894/cutover-after.log 2>&1
```

It exited **0**: six tests, all 18 rows, including the assertion that scoped
Inbox-only starts issue no hidden history, task, Files or Git reads. The harness
SHA remained identical to the successful baseline. The comparison parser also
exited **0** with no increase in any small-profile logical/counter gate.

This candidate was measured as uncommitted working source based on `e1c4b21f`,
with the warmer/runtime implementation and developing task/directory paging
changes present. It is not attributed to a later final commit. The synthetic
peers remain wire 3.33/3.34, so the new wire 3.35 task/directory opt-ins are not
exercised. [`cutover-manifest.json`](cutover-manifest.json) pins the working
patch and warmer source hashes; the full source-hash map remains in scratch.

Raw candidate records are [`cutover-after.jsonl`](cutover-after.jsonl). The
full 18-row comparison is in
[`cutover-comparison/cutover.md`](cutover-comparison/cutover.md), with detailed
publication/counter/wall observations in
[`cutover-comparison/cutover.csv`](cutover-comparison/cutover.csv).

| Scoped profile/start | Response bytes | RPCs | Logical ms | IDB callbacks |
| --- | ---: | ---: | ---: | ---: |
| small cold | 61,029 → 8,681 | 23 → 3 | 4,300 → 300 | 580 → 254 |
| small warm | 6,339 → 4,105 | 16 → 2 | 3,400 → 200 | 472 → 158 |
| small long absent | 11,052 → 8,818 | 18 → 4 | 3,600 → 400 | 656 → 336 |
| single-project cold | 2,006,126 → 199,279 | 247 → 5 | 52,450 → 500 | 3,844 → 1,602 |
| single-project warm | 103,005 → 92,221 | 195 → 2 | 42,550 → 200 | 3,011 → 1,005 |
| single-project long absent | 210,200 → 199,416 | 199 → 6 | 42,950 → 600 | 4,490 → 2,481 |
| large cold | 6,693,967 → 666,757 | 826 → 10 | 175,800 → 1,000 | 13,166 → 5,342 |
| large warm | 346,367 → 306,253 | 650 → 2 | 143,000 → 200 | 10,338 → 3,282 |
| large long absent | 707,008 → 666,894 | 659 → 11 | 143,900 → 1,100 | 15,296 → 8,216 |

All nine wire-3.33 cells retain exactly the same response bytes, RPC counts,
logical time and IDB callback counts. Logical row-publication latency remains
unchanged in all cells: 0 ms on warm updates and small starts; 200 ms/700 ms on
single-project/large scoped snapshots, which wait for the remaining pages
before their atomic cache publication. The scoped small profile's observed
cache wall time also decreases in all starts. Legacy wall samples vary, including
89.885 → 102.982 ms for small cold; unchanged deterministic work does not turn
that one wall observation into a statistical no-regression claim.

## Prepared final-source cohorts (not yet executed)

The controlled final runner currently pins code source
`18749eab6758df54dd96b2dcfde779b6176b99fd`. This preparation is not an execution
receipt. It requires the tracked SPA and complete production source tree to
match that commit before starting, and refuses an existing run directory.
The pending runner is
`/home/zech/Projects/.agent-scratch/894/run-final-cutover.py`; its prepared
SHA-256 is `c7580cec890300bddd52ad17ed3c6820f1ee64dc37825557be0af2a940431809`.

It serializes five separately logged fixture processes:

1. Wire-3.35 startup compatibility against exact `e1c4b21f` production SPA
   source: small/single-project/large × cold/warm/long-absent, nine records.
2. Exact-base selected-task mount: 20/4096 historical comments × cold/warm,
   four records, counting every selected task scope/detail RPC during mount.
3. Wire-3.35 startup compatibility against the pinned candidate, nine records.
4. Candidate selected-task mount, the same four cases.
5. The original frozen 18-row wire-3.33/3.34 startup harness against candidate.

The runner saves each affected candidate source using actual `cp`/`cmp`,
loads baseline Git objects without checkout, and verifies the **entire**
baseline production tree including unchanged files against its Git blob IDs.
Steps 1–2 share this controlled baseline window. A mandatory `finally` restores
every candidate file **from the saved copy**, touches it, and runs actual
`cmp`; complete non-ignored SPA file hashes/modes are checked before a failing
baseline gate can stop the run. Candidate-only files are backed up before
temporary removal, and only explicitly known baseline-only additions may be
removed during restore. Each later process rechecks source/driver identity.
There are no automatic measurement retries. Node 22.23.4 is named explicitly
both in the process argv and in its PATH, with `nice -n 10` and
`CARGO_BUILD_JOBS=8`. Each actual command has append-only start and completion
records with its exit code.

The compatibility-only clone, `compat335.test.js`, is separately frozen at
SHA-256 `8cbcaa79e71e703ae70ac2ff76182057b17d8fab58679f5b1d63db994c88bea5`.
Its synthetic 3.35 greeting advertises `tasks.timelinePaged`, `fs.treePaged`
and `changes.filesHints`. The tested SPA's real adapter selection/parser
decides which flags it recognizes; raw greetings and recognized new flags are
recorded with every profile. This tests old/new source against a simulated
new peer, not an installed bridge or opened task/tree contract.

The mounted opener, `taskOpenCost.test.js`, is frozen at
SHA-256 `bd035edd6b173bf54401b69a8743c12bff57691c414b71633dd155dbcd80fdf3`.
It mounts the real task page with the real scoped runtime/registry/engine and
IndexedDB task writers. It precompletes device bootstrap, holds replies long
enough to witness cached paint, and records all mount RPC bytes/counts,
publication counts and first-paint/data-publication wall observations. The
service, greeting and push-subscription wire are synthetic; read marks, review
metadata, user actions, SQL/disk and whole-app bootstrap are excluded. These
four-case mounted measurements remain pending; the one-endpoint byte probe in
`task-query-performance.md` must not be reported as this whole mounted opener.
