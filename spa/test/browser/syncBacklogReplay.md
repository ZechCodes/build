# Board-driven backlog replay (#850)

`syncBacklogReplay.test.js` is the short burst regression. It uses the existing
isolated headless Chromium/Vite fixture with real production cache sync,
IndexedDB, feed subscription, and thread HTML renderer. No authenticated device,
user browser, or live bridge participates. The RPC boundary is a seeded in-memory
server with 15 ms replies. Replay module loading has a 60-second startup bound
for the historical module graph on a busy host; timing starts after seeding and
module loading.

The deterministic mixed fixture has six project rows and 82 agents, 117 padding
feed rows with corresponding row records, 727 unrelated cache records, and eight unrelated terminal notification
keys. Every frame contains a revision-only board hint, a row/roster topic update
with thread tips, and an unrelated notification. Every 240 frames it also removes
a seeded row and status record and resets a separate seeded conversation. Missing
inline deltas exercise forward page repair against the in-memory server. Real
removal and reset barriers must preserve the exact final records and every
expected message. Six real watched-task project followers and the real agent-lineage reader are
mounted, so their existing production subscriptions exercise repeated feed
projections. The mounted conversation writes its DOM only after reading its
cache announcement, using the production thread renderer.

The short test deliberately holds an IndexedDB write transaction for 500 ms and
sends 240 frames at a requested 1 ms interval. Its body backlog assertion fails
against b33f5250 (651 pending in the initial recorded red run before mounting
the additional affected cache consumers). The test checks both body
and enrichment backlog, exact final messages/row states, reset clearing, removal
and full drain. CI also enforces a 2,048-row-get budget, at most 128 queue-job
starts after input ends before draining, and at most 128 queue-job starts from
any mounted message's push to its completed animation frame. Both body and
enrichment jobs count as turns. All 40 mounted render samples must complete;
all queues, active appliers, and transactions must be empty. These generous
work-count margins do not depend on CPU time granted by a loaded host. The
120-second drain deadline and 240-second test timeout are hang watchdogs, not
latency thresholds. The b33f5250 run of this exact 240-frame fixture made
419,801 row gets and failed the 2,048 count budget (exit 1). To reproduce the
baseline red:

```sh
nice -n 10 env SYNC_REPLAY_REVISION=b33f5250 npx vitest run test/browser/syncBacklogReplay.test.js
nice -n 10 npx vitest run test/browser/syncBacklogReplay.test.js
```

The acceptance profiler uses identical deterministic input for both revisions:
1,200 frames, requested 250 ms intervals (five minutes, 4 frames/second and just
over 12 items/second), without a deliberate storage stall. Run sequentially from
`spa/` on the pinned Node version:

```sh
nice -n 10 node test/browser/profileSyncBacklog.mjs b33f5250 /tmp/850-before.json
nice -n 10 node test/browser/profileSyncBacklog.mjs current /tmp/850-after.json /tmp/850-before.json
```

The second command exits nonzero if any acceptance bound fails: measured input
rate, mounted push-to-DOM-frame p95 <=1 second/max <=3 seconds, body pending <=64,
nonpositive endpoint backlog slope over the final two input minutes, drain <=5
seconds, >=80% fewer IndexedDB row `get` calls, exact messages/rows, reset/removal,
and empty final queues. The post-input drain bound includes delivery promises and work polling; an
unsettled delivery is reported as `deliveryDeadlineReached`, with its final
queue snapshot and incomplete-state checks preserved. JSON reports every latency and one-second backlog samples
so a reader can inspect more than the endpoint slope. Wall-clock timing includes
shared-host CPU scheduling. It does not claim virtual time or exclude incidental
machine delays. The only deliberate storage stall is in the separate burst test.

`rowGets` counts actual `IDBObjectStore.get` requests whose encoded cache address
has kind `row`, measured after seeding. This replay mounts taskFeed, six watched-task project followers, the lineage
reader, and a conversation renderer; it does not mount every route, task
page, rail DOM component, or console. Their shared changed-address reads are additionally
covered by `feedCacheProjection.test.js`. Source/session/epoch retirement and
actual user-action commit independence remain integration tests in the targeted
cache-sync and deferred-fence suites, rather than being simulated by this replay.

## Recorded acceptance evidence

The sequential October 25 run used b33f5250 for all baseline production modules
and the task branch's final queue/projection implementation for the after run.
Both sent all 1,200 identical mixed frames and met the required measured rates.
There was no injected storage stall in either run.

| Measurement | Before | After |
| --- | ---: | ---: |
| Measured frames/s | 3.500638 | 3.932134 |
| Measured items/s | 10.531085 | 11.829170 |
| IndexedDB row gets | 2,092,257 | 3,030 |
| Mounted push-to-frame p95 | 237,522.6 ms | 76.2 ms |
| Mounted push-to-frame max | 250,727.8 ms | 246.7 ms |
| Peak pending body jobs | 1,813 | 8 |
| Peak pending enrichment jobs | 1 | 0 |
| Final-two-minute body slope | +5.297302 jobs/s | -0.008419 jobs/s |
| Final-two-minute enrichment slope | 0 | 0 |
| Drain after final input | 272,687.7 ms | 55.5 ms |
| Exact final messages/rows | true | true |

Row gets fell 99.85518%. Both final queues were empty. The after run also passed
seeded removal/reset checks and all eight final terminal snapshots. Raw reports
are `/tmp/850-before-acceptance.json` and `/tmp/850-after-acceptance.json`; the
comparison command exited zero. The before runner awaited all delivery promises
before its polling deadline, allowing its full 272.7-second drain to be measured.
That runner weakness was fixed before the after run: the deadline now covers the
delivery promises too. The emitted load and measured counters did not change.
Final terminal verification was added after the baseline started; it executes
after counter restoration and has no effect on measured load or latency.

The separate after-only stall capture (`/tmp/850-burst-after.json`) sends 240
mixed frames at requested 1 ms intervals while a real IndexedDB write transaction
is deliberately held for 500 ms. Including that injected stall, mounted latency
was p95 836.6 ms/max 876.5 ms, peak pending body jobs 41, and drain 1,406.5 ms.
Exact final messages/rows passed and the capture command exited zero. These
numbers are separate from the steady-load acceptance table above.
