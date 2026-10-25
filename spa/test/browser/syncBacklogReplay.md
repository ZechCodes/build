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
and full drain. To reproduce the baseline red:

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
