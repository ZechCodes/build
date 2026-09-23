# Test suite audit — #121

## Running the gates

Run `npm run lint`, plain `npx vitest run`, and `npm run build` in `spa/`.
Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`, and `cargo build --release` in `bridge/`. Read each process's
exit code; a green test summary does not clear an unhandled Vitest error.

The SPA uses a sibling `build-secure-transport/js` dependency. Install into
this workspace with `npm install --legacy-peer-deps` after changing
`spa/package.json`. The lockfile is ignored; do not use `npm ci` or share
another checkout's `node_modules`.

Vitest keeps file isolation and its default fork pool. The node/jsdom project
uses half the available CPUs, capped at twelve workers. The Chromium layout
project runs afterwards with at most two workers. Both are included in an ordinary
`npx vitest run`; there is no retry or known-flaky exclusion. The layout
server uses its own Vite dependency cache and explicitly prebundles the Prism
grammars and terminal dependency before serving their optimized imports.
If a new CommonJS browser dependency is added, include it in the layout
harness's prebundle list. Dependency discovery is disabled there to prevent
invalidating module requests already in flight. `libsodium-wrappers` stays
outside that optimizer so Vite's existing ESM resolver shim can handle it.
The harness forwards loopback asset requests through Playwright's HTTP client
to the real Vite server, without retries or response mocks. This keeps host
network-change notifications from cancelling Chromium's module imports with
`ERR_NETWORK_CHANGED`; server failures still propagate to the browser.
Routing disables Chromium's HTTP cache; these checks cover real renderers and
IndexedDB behavior, not browser HTTP caching.

Layout screenshots are optional review artifacts, not assertions. Set
`BUILD_LAYOUT_SCREENSHOTS` to an output directory when regenerating them;
ordinary gates avoid the capture work and leave tracked images unchanged.

Keep each workspace's Cargo target cache across gate runs. The baseline had
to rebuild current sources despite an existing target cache; report compile
time separately from test execution. No nextest dependency or Cargo profile
change is needed for the test fixes here.

## Coverage-preserving cuts

| Removed check | Reason | Retained coverage |
| --- | --- | --- |
| Four exact CSS substrings in `userMessageTicks.test.js` (nav position/inset, tick width, timeline padding) | Pin implementation spelling rather than geometry. | `browser/userTickLayout.test.js`: “the grouped ticks consume only the chat's existing left padding” measures the real gutter, pill bounds, and unchanged message width; the DOM navigator test remains. |
| `agentSurfacesRender.test.js`: “is the one place the viewer's clip markup is built” | Counts source tokens and bans a constant name without exercising output. | The adjacent “is one span…” and “shows one line…” cases check clip markup, title, line count and escaping; “clips the summary label but leaves expanded agent details readable in full” checks expanded output. |
| The `modelLabel` source substring assertion in `agentSurfacesRender.test.js` | Duplicates the rendered-label assertion in the same test. | “prints the model name the row arrived with” still asserts the resulting label. |

One test and five additional assertions were removed. The now-unused
`EXPANDED_ATTRIBUTE` import was removed with its source-only test. No real
browser test, unmocked feature wiring, or behavior assertion was deleted.

The fixture audit found no unused SPA helper modules or bridge fixture files.
SPA capture scripts are manual tools, not unused fixtures. Bridge's presence
challenge, Pi scripts, agent surface fixture, Claude/Codex stream corpus and
provenance checks all have callers. Shared relay, Git and off-lock helpers
remain in use. `mockedExports.test.js` remains: it catches invented mock
exports against the real module exports, a previously observed build failure.

## Deterministic synchronization

- Modal animation assertions advance a fake clock and finish the recorded
  animations; focus preservation is checked after entry motion finishes.
  The shared recorder also awaits its actual timeout when a suite fakes only
  intervals, so it cannot finish its polling rounds before real motion starts.
- Settings actions, workspace modal closure, and tracker card moves wait for
  their DOM, RPC or error outcome. A refused move waits for rollback/error
  completion instead of assuming twenty event-loop ticks finish IndexedDB.
- Cut activity runs in the agent rail wait for the fetched sequence rows and
  fold state before checking navigation and fetch deduplication.
- The bridge's unauthenticated socket test waits for a real ping/pong before
  asserting unreachability. The saturated-worker test holds its worker on a
  channel until the liveness assertion, then releases it even on panic.
- Shutdown checks wait for the conversation's session-end record and activity
  stream closure. A tab becoming non-live and a provider status becoming Ended
  precede those effects; they are not completion barriers for later assertions.
- The fixture-gated migration test sets its record's mtime relative to the
  import note instead of sleeping for filesystem timestamp resolution.

The bridge's real heartbeat, silence, and RTC teardown windows are retained;
they test elapsed liveness boundaries. Broad rewrites of unrelated zero-delay
flush helpers are outside this patch. The cached-projects-folder test already
waited on the DOM value, so it relies on the bounded suite concurrency rather
than a new sleep or a larger per-test timeout.

## Measurements

The issue attachments contain all per-file durations, the slowest 30 files,
test-executable timings and exact command exit codes. File durations are
Vitest's test/hook durations; adding parallel file times does not give suite
wall time.

Baseline `ee9c4bda8e171d4cbe4b153620349f75a1b8b48a`, Node 26.8.1,
Vitest 4.1.11, Rust 1.98.1, 24 logical CPUs, with cargo and Vitest together:

| Suite | Wall time | Exit | Detail |
| --- | ---: | ---: | --- |
| SPA | 145.807 s | 1 | 382 files / 6599 tests; four browser timeouts and 15 optimizer errors |
| Bridge | 238.664 s | 0 | 148 s compiling; 81.88 s summed executable test durations |

The SPA split was 166 node files / 22.061 s, 210 jsdom files / 480.817 s,
and six browser files / 156.475 s (aggregate file durations). The two baseline
attempts interrupted by the Build restart produced no exit codes and are
excluded. The bridge `device_presence` executable fell from 6.01 s to 1.01 s
in targeted checks without changing its four tests. The ignored migration
test was compiled but needs `BUILD_MIGRATION_FIXTURE` to execute.
