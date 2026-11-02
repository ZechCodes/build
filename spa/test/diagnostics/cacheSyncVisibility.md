# Proposed visibility-only diagnostic reservation

Preparation only: this diagnostic successor is unexecuted. Frozen 4bd and the
R-909-C/D records remain unchanged. R-909-D's subscription-readiness evidence,
simulated-a805 RED and after-commit detection limitations are not extended by
this proposal. No new production or normal fixture-helper behavior is added.

## Three distinct focused cases in one new invocation

`READINESS_MODE=visibility` injects only these three named cases into the
existing fixture scope, using its actual helper and production event path:

1. `visibility-pending`: hold the real background subscribe response on the
   fresh cache; verify true initial/pre-ack pass completion while readiness is
   pending. Dispatch a real, cancelable visibilitychange with document.hidden
   returning false. Require the actual guard to reject readiness with its
   specific error and invalidate assertCurrent. The event must remain
   uncancelled and reach an independent window ancestor listener in bubbling
   phase. Capture the actual production
   visibility pass, release background and require successful pass completion.
2. `visibility-sealed`: reach the actual fulfilled seal, dispatch the same real
   visible event and require the guard's specific assertCurrent rejection.
   The original readiness promise must remain fulfilled with the same seal:
   this is later invalidation, not retroactive rejection or prevention. The
   event still reaches the independent listener and production pass, which
   must complete true.
3. `visibility-disposed`: reach readiness and dispose the helper without
   stopping production sync. Verify that the exact capture listener is removed
   with matching identity/options. A subsequent real event must cause zero
   guard calls, yet still reach the independent listener and start/complete
   the actual production visibility pass. This establishes removal and
   noninterference rather than merely asserting removeEventListener was called.

The diagnostic interposes only on capture=true visibility registration/removal:
it forwards native operations with the same options and wraps the actual guard
to count invocations while preserving its this/event arguments. Original and
wrapper function identities are recorded. The matching native removal must
remove that wrapper; wrong identity/options cannot satisfy the post-disposal
zero-hit assertion. Production's noncapture handler is not replaced. The
visibility getter, independent window listener, wrappers and gates are restored or
removed in both normal and timeout cleanup. These are deliberate controlled
diagnostic inputs, not full-suite timing/environment equivalence.

Each case retains actual guard registration/removal/call identities, event and
production-pass identity, dispatch/defaultPrevented and listener-call counts,
actual pass outcomes, errors and terminal markers. Reuse the existing bounded
128-event/28-KiB payload and 32-KiB artifact budget, fsynced file/directory and
digest receipts. No loss/missing hook/missing receipt or substituted pass result
can qualify as green. Raw test failure is retained separately from capture
failure. The future driver verifies these identities/order plus exact head,
source hash and mode. False pass results fail qualification.

## Bounded commands for a separate reservation

Proposed maximum: **three sequential install/test admissions**, one worker,
nice -n 10, no retry. One visibility invocation has exactly three cases. This
is a separate proposal, not an amendment to ended R-909-D's two-case bounds.
Request a five-minute window with a hard minute-three admission cutoff; any
admitted child finishes naturally. Preserve the original 60-second hook and
20-second per-test budgets; no sleeps, timeout expansion, browser, build, scan,
readiness RED/GREEN rerun, original-case rerun or full-file rerun is included.

Pinned tools:

- Node `/home/zech/.local/share/mise/installs/node/22.23.4/bin/node`.
- npm CLI `/home/zech/.cache/node/corepack/v1/npm/12.0.2/bin/npm-cli.js`.

The three proposed gates are:

1. Root: `nice -n 10 NODE NPMCLI ci --prefix transport/js --ignore-scripts --loglevel verbose`.
2. SPA: `nice -n 10 NODE NPMCLI ci --legacy-peer-deps --ignore-scripts --loglevel verbose`.
3. SPA, visibility mode: `nice -n 10 NODE node_modules/vitest/vitest.mjs run --project unit --maxWorkers 1 --config test/diagnostics/cacheSyncReadiness.config.mjs test/cacheSyncDelivery.test.js -t '909 visibility diagnostic'`.

Reviewable launcher, only after exact-head assessment and a NEW unique START:

```
python3 spa/test/diagnostics/run-cache-readiness.py --execute --plan visibility \
  --reservation NEW_RESERVATION --start NEW_UTC_START \
  --cutoff NEW_UTC_MINUTE3_CUTOFF --head REVIEWED_IMMUTABLE_SUCCESSOR_SHA \
  --root /home/zech/Projects/.build-worktrees/workspaces/proj-1/909-visibility-diagnostic/Build \
  --artifacts /tmp/NEW_UNIQUE_VISIBILITY_ARTIFACT_DIRECTORY
```

The launcher preserves the older readiness plan as its default; this proposal
requires explicit `--plan visibility`. All three expected raw exits are 0.
Setup/qualification/test failure, source change, competing work or cutoff stops
later admissions. Record skipped stages and nonzero final status for any
incomplete/stopped sequence. Natural owned-child cleanup and a fresh check
precede END. Read-only HEAD/cleanliness/inventory checks are metadata, not extra
test variants. No source/driver refinement or admission extension is implicit.

The original conservative executable inventory remains. Before scheduling,
request reuse of the previously reviewed desktop-only exclusion if necessary:
reverify its exact PID/start-time/SID/executable and absence of headless/debug
test flags; retain its external diff/hash and never exempt Chrome or Node
broadly. New or unresolved competing work still blocks. This proposal alone
does not authorize that refinement or any launch.

Results, if successful, would support only the three existing visibility guard
paths and cleanup. They would not attribute historic full-gate failures,
resolve workspace/phone observations, release native/E2 ownership, establish
full-suite parity or change #930's broader verdict without independent review.
