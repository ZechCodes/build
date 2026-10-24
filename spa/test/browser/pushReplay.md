# Controlled push replay

Run from `spa/` after installing transport and SPA dependencies:

```sh
nice -n 10 npx vitest run test/browser/pushReplay.test.js
nice -n 10 node test/browser/profilePushReplay.mjs 5c04bfa7834c0832be109253d15308058039f9b9 /tmp/push-replay-before.json
nice -n 10 node test/browser/profilePushReplay.mjs current /tmp/push-replay-after.json
```

Each run starts its own Vite server and headless Chromium with fresh isolated
storage, then closes both. The baseline option serves **all production source
modules** from the selected Git revision; the test fixture, event sequence,
storage seed, replay cadence and simulated RPC remain identical. It does not
open a bridge, authenticate, or connect to an existing browser.

The seed has 818 records, six projects and 82 agents. Its 123-item feed and
82-entry workspace list have similar sizes to #816; exact serialized sizes are
included in the report. Sixty frames carry concurrent agent state changes,
contiguous inline messages and already-covered empty tips across six projects.
The profile uses the observed 60 frames/17.3 seconds cadence, a deliberate
2-second real IndexedDB write-lock stall and fixed 15ms mocked RPC latency.
The regression compresses the cadence and stall to exercise overlapping work
without slowing every browser suite. Its conversation uses the production
thread HTML renderer, a cache subscription and animation-frame completion;
the production feed is subscribed concurrently.

Reports count production IndexedDB transactions and gets (excluding the stall
fixture's keep-alive requests), peak unfinished transactions and active item
appliers, scheduler work where available, RPC count, event-to-cache and
cache-subscribed render latency, foreground applier-turn delay, and exact final
sequence/state checks. Cache
latency includes the deliberate stall. `renderProgressDuringReplay` proves
visible cache-driven progress before incoming traffic ends. Verification reads
are outside the measurement window.

This is a controlled synthetic comparison, not a replay of private captured
frames or a claim to reproduce the original live rates. The observed #816
baseline used different event mix, real RPC timings and other browser activity.
In particular, this fixture deliberately avoids cursor holes: catch-up cursor
and repair behavior belongs to #819.

The regression requires the open conversation to reach cache within three
applier starts after arrival, while unrelated projects continue to update.
This scheduling bound is independent of CPU scheduling on the shared host;
the separate 10-second wall ceiling detects a hang. Profile reports retain
raw wall latency, including the deliberately stalled transaction.
