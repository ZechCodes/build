# Build web app (Skrift)

A minimal [Skrift](https://github.com/ZechCodes/skrift) app that provides **auth**
and serves the Build SPA. After login you write a goal → a planning agent writes a
plan → you approve → a coding agent implements it → you watch the git diff appear
live. The SPA talks to the bridge over E2EE via the gateway; Skrift only does auth
+ serving.

- `app.dev.yaml` — controllers (BuildController + Skrift auth/admin), sqlite, dummy
  auth, and a dev-relaxed CSP so the SPA can use esm.sh + the gateway WebSocket.
- `buildapp/controllers.py` — `BuildController` at `/app`: requires a Skrift session
  (else redirects to `/auth/login`), then serves `build.html`.
- `buildapp/build.html` — the SPA (goal → plan → approve → live diffs), connecting
  to the gateway at `ws://localhost:18090`.

## Run

```bash
# 0. Bridge in real-agent mode (claude on PATH) + gateway/relay up — see deploy/.
# 1. Secret + first-run setup (one time):
echo "SECRET_KEY=$(python3 -c 'import secrets;print(secrets.token_urlsafe(32))')" > .env
SKRIFT_ENV=dev uv run --project /path/to/Skrift skrift db upgrade head   # migrations
#    (migrations land in the Skrift checkout's app.db; copy it here, then seed setup-complete)
cp /path/to/Skrift/app.db ./app.db
sqlite3 ./app.db "INSERT INTO settings (id,key,value,created_at,updated_at) \
  VALUES (randomblob(16),'setup_completed_at','done',datetime('now'),datetime('now'));"

# 2. Serve
SKRIFT_ENV=dev uv run --project /path/to/Skrift skrift serve   # :8080
```

Open <http://localhost:8080/app/> → log in with **Dummy Login (Dev)** (any email) →
write a goal → Create plan → Approve plan & build → watch the diff.

Automated end-to-end: `node ../web/skrift-flow.mjs` (Playwright: login → full flow →
screenshots to /tmp/build-app-*.png).

## Production

`app.yaml` is the real production config (loaded when `SKRIFT_ENV` is unset or
`production`): passkey-only auth pinned to `https://getbuild.ing`, rate limiting
on, env-driven Postgres (`$DATABASE_URL`), and a same-origin CSP whose only
external target is `wss://relay.getbuild.ing`. Dummy auth, the relaxed CSP, and
the `/internal/*` localhost fallback live only in `app.dev.yaml`.

Container (migrates then serves — `skrift db upgrade head` runs framework +
app migrations against `$DATABASE_URL` on every start):

```bash
podman build -t ghcr.io/8ly-dev/build-app -f Containerfile .
podman run -e SECRET_KEY=... -e INTERNAL_API_SECRET=... \
  -e DATABASE_URL=postgresql+asyncpg://user:pass@host:5432/db \
  -p 8080:8080 ghcr.io/8ly-dev/build-app
```

Required env: `SECRET_KEY`, `DATABASE_URL`, and `INTERNAL_API_SECRET` — the
shared secret the relay must present as `X-Internal-Secret` on `/internal/*`
(see `buildapp/internal_auth.py`).

The one-time setup is normally the Skrift web wizard (a fresh deploy serves
`/setup` until completed); the steps above seed it non-interactively.

Tests: `uv run pytest buildapp/` (from this directory).
