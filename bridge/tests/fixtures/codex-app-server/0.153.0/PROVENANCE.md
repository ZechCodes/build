# Codex app-server 0.153.0 fixtures

`observed-handshake.jsonl` is an initialize-only local capture from the installed
`codex-cli 0.153.0` binary on 2026-09-03. Home paths, host names, installation
ids, timestamps, and operating-system patch versions were replaced with fixed
placeholders. The probe opened no thread and made no model or network request.

`synthetic-model-events.jsonl` is hand-written from the 0.153.0 generated JSON
schemas because collecting authenticated model events was unnecessary and could
expose account or workspace data. It contains no credentials or machine values.

Generated schema SHA-256:

- `codex_app_server_protocol.v2.schemas.json`: `d3eace08be5dca386bfd1f1e8df650058b4113f1e10870a284d775d75517576a`
- `v1/InitializeResponse.json`: `62ad689c2cb6379913c1d72749cfd8de5089d35760214123518eb92eef11acc9`
- `v2/ThreadStartParams.json`: `792e2f32e37cece971bd616664ea2053741acbed4e9c92e9d1766427718f2ecd`
