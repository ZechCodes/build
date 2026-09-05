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

## Production

`app.yaml` is the real production config (loaded when `SKRIFT_ENV` is unset or
`production`): passkey-only auth pinned to `https://getbuild.ing` (dummy auth is
dev-only; Skrift hard-blocks it in production), rate limiting on, env-driven
Postgres (`$DATABASE_URL`), and a same-origin CSP whose only external target is
`wss://relay.getbuild.ing`. Dummy auth, the relaxed CSP, and the `/internal/*`
localhost fallback live only in `app.dev.yaml`.

The SPA bundle is served same-origin; build it with
`VITE_RELAY_URL=wss://relay.getbuild.ing` for production.

Container (migrates then serves — `skrift db upgrade heads` runs framework +
app migrations against `$DATABASE_URL` on every start):

```bash
podman build -t ghcr.io/8ly-dev/build-app -f skriftapp/Containerfile .   # from the repo root
podman run -e SECRET_KEY=... -e INTERNAL_API_SECRET=... \
  -e DATABASE_URL=postgresql+asyncpg://user:pass@host:5432/db \
  -p 8080:8080 ghcr.io/8ly-dev/build-app
```

Required env: `SECRET_KEY`, `DATABASE_URL`, `INTERNAL_API_SECRET` — the shared
secret the relay must present as `X-Internal-Secret` on `/internal/*` (see
`buildapp/internal_auth.py`) — plus the four outbound-email variables:
`SMTP_USERNAME` (the FastMail login address), `SMTP_PASSWORD` (a FastMail
app password), `SMTP_FROM_ADDRESS` (the address mail is sent from, e.g.
`Build <hello@getbuild.ing>`), and `WAITLIST_NOTIFY_ADDRESS` (where a new-signup
notification goes; empty disables it). `app.yaml` interpolates the three `SMTP_*`
variables and the app refuses to boot without them.

### Email

`SKRIFT_ENV=dev` uses the console backend: outbound mail is logged instead of
sent, so dev needs no credentials at all. To send real mail from a local server,
put `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM_ADDRESS` and
`WAITLIST_NOTIFY_ADDRESS` in `skriftapp/.env` (gitignored) and run:

```bash
SKRIFT_ENV=mail uv run --project /path/to/Skrift skrift serve --port 8090
```

`app.mail.yaml` is the dev config plus the production SMTP block, with the
public base URL pointed at `http://localhost:8090` so unsubscribe links in the
mail land back on the local server that signed them.

FastMail: `smtp.fastmail.com:587` with STARTTLS; the username is the login
address, the password is an app password (not the account password), and the
from address must be an address or alias on that account.

The one-time setup is normally the Skrift web wizard (a fresh deploy serves
`/setup` until completed); the steps above seed it non-interactively.

Tests: `uv run --frozen pytest buildapp` (from this directory).
Lint: `uv run --frozen ruff check buildapp` — correctness plus the C901
complexity cap (CLAUDE.md “Complexity gates”).
