# Phone reconnect replay (#134)

Run from the repository root with the SPA dependencies installed. This uses
real local app, relay, bridge, WebRTC and IndexedDB, plus system Chromium at
`/usr/bin/chromium`. Ports 8090, 18090 and 8134 must be free. Use your own
compose project name; the script restarts that project's relay only.

```sh
export COMPOSE_PROJECT_NAME=phone134
nice -n 10 docker compose -p "$COMPOSE_PROJECT_NAME" -f deploy/compose.real.yml up -d --build
API_URL=http://localhost:8090 PAIRING_CODE=COMPOSE-PAIR nice -n 10 node web/pair.mjs
nice -n 10 node scripts/replay-phone-reconnect.mjs > /tmp/phone134-replay.log 2>&1
nice -n 10 docker compose -p "$COMPOSE_PROJECT_NAME" -f deploy/compose.real.yml down
```

The default Playwright import is the SPA's installed `playwright-core`.
To use the originally requested installation, set `PLAYWRIGHT_MODULE` to
`~/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs`
(expand `~` when setting the variable). `REPLAY_OUTPUT` changes the artifact
directory, which defaults to `/tmp/phone134-replay`.

The fixture creates a workspace, conversation agent and task in the disposable
compose bridge. Vite serves the current SPA source on 8134 and proxies its API
to the compose app. An observation-only transform logs RPC method names.
After confirming a populated view, it marks the page hidden, takes Chromium
offline for 35 seconds, closes its actual IDB handles and peer connections,
then dispatches the visibility/pageshow/online wake events. It checks that
cached content remains, capabilities match, storage accepts new writes and a
new agent push reaches the cache and rail. A separate relay restart must also
leave the workspace populated. No hard refresh is used.

Exit 0 means the assertions passed. The output directory holds screenshots,
state/diagnostic snapshots and bridge logs; stdout contains browser console
and RPC traces. This deliberately injects browser storage loss in Chromium;
it is not physical iPhone verification. The deterministic same-session and
late-continuation cases are covered by `spa/test/cacheSync.test.js`, and
transient IDB write failures by `spa/test/localCache.test.js`.
