# Provisional cache fixture readiness experiment

This is unexecuted review preparation. No red/green outcome, fixture acceptance,
historical full-gate attribution or workspace diagnosis is claimed.

The normal `cacheSyncDelivery.test.js` change is local to this file's mock wire
and one push stand-down case. It captures the actual synchronous initial pass,
requires successful initial/successor results and current-session manager
acknowledgments of the exact inbox/background specs, drains the subscription
chain, and seals a stable coverage generation with no pass outstanding. It
checks that seal before and after the unchanged seed and before the first push.
Later invalidation is a setup failure; it cannot undo an admitted transaction.
The original four assertions, file-read gate timing, hand-back, other settle
calls and runner deadlines remain. Normal finally and afterEach dispose the
observer and release gates, including after a timeout.

The controller and configuration here are **opt-in diagnostics**, never imported
by the normal test file. The config injects two cases into the existing describe
scope. It does not transform production modules, supply pass results, dispatch
synthetic held events, or add a timeout/sleep. It selects the unit project only;
layout warmup is absent. This is an explicit diagnostic configuration difference,
not full-suite parity. The subsequent original-case/full-file commands use the
normal checked-in config and select unit in the usual way.

## The two cases in each of two matched invocations

1. `delayed-landing`: withhold the actual background subscribe response on an
   empty cache; require true initial and already-started successor completions,
   actual inbox acknowledgment, and no pass outstanding while the background
   manager acknowledgment is still absent. The wire's early held set must
   already contain background, demonstrating why it alone is insufficient.
   RED deliberately selects a805's `passesSettled` barrier semantics on the
   same provisional candidate with matched passive observers, and commits the
   seed before acknowledgment. It is **not an exact a805-head gate run**.
   GREEN must keep both seed attempt and canonical seed absent.

   Arm the run-1 `git.unpushed` response gate only after the pre-ack work is
   complete. Release background, observe its actual manager acknowledgment and
   the real checkout successor read. Keep that read held while an actual
   same-session `greetBridge` replay advances the required subscription
   generation during barrier formation. Require readiness still pending.
   Release the read and require successful completion, current coverage and
   stable generation. GREEN may now seed and execute the four original
   count/eviction/hand-back assertions. RED fails with the specific
   `EXPECTED_READINESS_RED` detector after retaining the required ordering.

2. `invalidation-after-commit`: the same candidate helper in BOTH modes reaches
   readiness, commits and canonically verifies the seed, then a real manager
   replay invalidates the seal. `assertCurrent` must reject and repair passes
   must settle true. This proves detection after commit, explicitly not seed
   prevention. It is inside the same diagnostic invocation, not a seventh
   admission or silently added variant.

The delayed response permitting initial completion and dispatching a checkout
successor is currently supported by source order only: fresh-cache
`heldBeforeReading` skips the chain wait; fixture project p1 has no source
terminal jobs; a late background landing repairs observed checkout reads and
the run scope always asks `git.unpushed`. The runtime must actually exhibit that
order. A different order, timeout, failed pass or missing capture is inconclusive
for this mechanism, never a reason to adjust the assertions or retry.

## Proposed fresh reservation

Nothing is scheduled. A new coordinator reservation ID, START and UTC cutoff
are mandatory; both retired windows are unusable. Ten minutes nominal, hard
minute-eight admission cutoff. At most six sequential **install/test gate
admissions**, one Vitest worker, no retries, builds, scans or extra variants:

1. Root: pinned Node/npm `ci --prefix transport/js --ignore-scripts --loglevel verbose`.
2. SPA: same `ci --legacy-peer-deps --ignore-scripts --loglevel verbose`.
3. SPA: pinned Node `node_modules/vitest/vitest.mjs run --project unit --maxWorkers 1 --config test/diagnostics/cacheSyncReadiness.config.mjs test/cacheSyncDelivery.test.js -t '909 readiness diagnostic'`, RED.
4. Same diagnostic command, GREEN.
5. SPA: pinned Node `node_modules/vitest/vitest.mjs run --project unit --maxWorkers 1 test/cacheSyncDelivery.test.js -t 'writes nothing back from a push read the sync layer stood down under'`.
6. SPA: pinned Node `node_modules/vitest/vitest.mjs run --project unit --maxWorkers 1 test/cacheSyncDelivery.test.js`.

Every gate is `nice -n 10`. Node is
`/home/zech/.local/share/mise/installs/node/22.23.4/bin/node`; npm CLI is
`/home/zech/.cache/node/corepack/v1/npm/12.0.2/bin/npm-cli.js`.
The launcher records binary/CLI hashes, npm verbose tool versions, diagnostic
Node version, exact argv, admission/exit times, raw exits and output. Its
read-only Git cleanliness/HEAD checks and `/proc` inventories are metadata
checks, not extra install/test variants. Possible active competing executable
states stop admission; suspended/zombie shell command strings are not gates.

The launcher is reviewable executable scheduling code, not an authorized job:

```
python3 spa/test/diagnostics/run-cache-readiness.py --execute \
  --reservation NEW_COORDINATOR_RESERVATION \
  --start NEW_UTC_START --cutoff NEW_UTC_MINUTE8_CUTOFF \
  --head REVIEWED_FULL_IMMUTABLE_SUCCESSOR_SHA \
  --root /home/zech/Projects/.build-worktrees/workspaces/proj-1/909-cache-readiness-successor/Build \
  --artifacts /tmp/NEW_UNIQUE_READINESS_ARTIFACT_DIRECTORY
```

Review the exact provisional commit before START. All six gates then use that
same immutable, clean successor: no intervening implementation/review commit is
assumed. The RED/ GREEN overlay results are separately labeled; stages 5/6
are the uninstrumented exact-head case/file validation. They are admitted only
after both diagnostic invocations meet their distinct raw-exit and completeness
criteria. If review requires a new implementation commit after those diagnostics,
stages 5/6 must be skipped and rescheduled against that new head; no silent
commit, head substitution or extra test is permitted within this plan.

Expected qualified RED exit is 1 with its specific detector and the second case
passing; GREEN/case/file/setup exits must be 0. Any unexpected result, incomplete
or malformed trace, competing work, source change, setup/case failure or cutoff
skips the rest. Ledger failure also stops admissions while admitted children
finish naturally. The driver awaits its actual process sessions, never kills
children or touches unrelated PIDs, and returns nonzero if incomplete/stopped.
Fresh owned-child inventory and the report must precede coordinator END.

Each case retains a bounded 128-event/28-KiB payload (32-KiB artifact budget),
source hash, head/mode/session/request/pass identities, actual request/response
and manager acknowledgment, seed attempt/canonical commit+write identity/hash,
generation changes, raw test error and terminal marker. File+directory flush
and a digest receipt are required; any lost event or missing coverage/receipt
makes the capture inconclusive. Timeout cleanup fences later actions, stops the
captured sync before gate release, and records an incomplete fallback even if
the async test body has not reached finally. Persistence errors never replace
the raw test error or become a qualified expected RED.
