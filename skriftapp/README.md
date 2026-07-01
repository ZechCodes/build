# Build web app (Skrift)

A minimal [Skrift](https://github.com/ZechCodes/skrift) app that provides **auth**,
the device registry/pairing api, gateway-token minting, and serves the Build SPA.
After login you write a goal → a planning agent writes a plan → you approve → a
coding agent implements it → you watch the git diff appear live. The SPA talks to
the bridge over E2EE **directly through the relay's `/ws/client`** (the node
gateway is retired); Skrift does auth + api + serving.

- `app.dev.yaml` — controllers (BuildController + devices api + Skrift auth/admin),
  sqlite, dummy auth, and a dev-relaxed CSP.
- `buildapp/controllers.py` — `BuildController` at `/app`: requires a Skrift session
  (else redirects to `/auth/login`), then serves the built SPA (`buildapp/static/`).
- `buildapp/devices_controller.py` — device register/lookup/approve/list/revoke,
  `POST /api/gateway-token`, and the relay-facing `/internal/*` endpoints.
- The SPA source lives in `../spa/` (Vite, vanilla ES modules, zero CDN). Build it
  with `cd ../spa && npm install && npm run build` → emits `buildapp/static/`.

## Run

```bash
# 0. Build the SPA bundle (one time, and after SPA changes):
( cd ../spa && npm install && npm run build )

# 1. Secret + first-run setup (one time):
echo "SECRET_KEY=$(python3 -c 'import secrets;print(secrets.token_urlsafe(32))')" > .env
SKRIFT_ENV=dev uv run --project /path/to/Skrift skrift db upgrade head   # migrations → ./app.db
sqlite3 ./app.db "INSERT INTO settings (id,key,value,created_at,updated_at) \
  VALUES (randomblob(16),'setup_completed_at','done',datetime('now'),datetime('now'));"

# 2. Serve the api + SPA
SKRIFT_ENV=dev uv run --project /path/to/Skrift skrift serve --port 8090   # :8090

# 3. Relay + bridge (separate shells)
RELAY_PORT=18090 RELAY_API_URL=http://127.0.0.1:8090 ../bridge/target/debug/relay
BRIDGE_API_URL=http://127.0.0.1:8090 BRIDGE_WEB_URL=http://127.0.0.1:8090 \
  BRIDGE_RELAY_URL=ws://127.0.0.1:18090 BRIDGE_IDENTITY_FILE=/tmp/bld/id.json \
  BRIDGE_REPO=/tmp/bld/repo BRIDGE_WORKTREES=/tmp/bld/wt \
  ../bridge/target/debug/build-bridge serve
```

Open <http://localhost:8090/app/> → log in with **Dummy Login (Dev)** (any email) →
pair the bridge with the pairing code it printed → write a goal → review the plan →
approve → watch the diff.

Automated end-to-end (from `../web/`):

- `API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node e2e.mjs` — E2EE round-trip
- `… node qa.mjs` — 16-check task-lifecycle suite (bridge needs `BRIDGE_QA_AGENT=1`)
- `APP=http://localhost:8090 node skrift-flow.mjs` — Playwright: login → goal →
  plan → approve → live diff, screenshots to /tmp/build-app-*.png
- Multi-device: set `PREFER_DEVICE_ID=<device uuid>` to pin the device under test,
  `QA_EMAIL=<email>` to log in as the device's owner.

## Production notes

- Dummy auth is dev-only (Skrift hard-blocks it in production); production uses
  Skrift's passkey method.
- The SPA bundle is served same-origin; build with
  `VITE_RELAY_URL=wss://relay.getbuild.ing` for production.
- The one-time setup is normally the Skrift web wizard; the steps above seed it
  non-interactively.
