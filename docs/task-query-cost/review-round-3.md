# Third review corrections

This round corrects copied-workspace review subscriptions, recovery from an
internal directory-window cursor refusal, and idle directory-lease cleanup.
The combined source is frozen at
`b38e11bc78fd0b6925352425e54b931a249101ba`. The JavaScript changes are committed
as `4bd271861` and `105b2e4a2`; `b38e11bc78` adds idle lease cleanup. Fresh
positive gates and all three behavioral controls completed with their expected
exits and verified source restoration. No performance cohort was rerun.
Secret scan/publication are recorded separately by the parent after the
artifact commit.

## Contract changes

Modern live review Files reads the selected workspace directory, while an
original `project_source` producer watches the original folder. It now calls
`directoryWatchSpec({workspace_id})`, retaining the existing board/state
registration solely for open, explicit refresh, reconnect and greeting
refresh. All change items are ignored, including original-source hints and
unrelated board/agent state. Refresh reads still check visibility, generation,
selected path and cache ownership. The legacy workspace/files branch remains
unchanged. Git review Files continues reading the saved commit.

The producer audit has three distinct cases:

| Existing producer | Actual coverage and limit |
| --- | --- |
| Plain-only workspace conversation run | Its container may recursively cover a copied plain directory. Using this requires verified owner/root metadata and container-to-directory path translation. That is follow-up work, not implemented here. |
| Mixed workspace conversation run | Its root resolves to the first Git directory, excluding copied plain siblings and other secondary directories. |
| Direct workspace-id subscription | No matching filesystem producer exists. Exact entity routing does not translate workspace ids to run ids. |

The source evidence is `bridge/src/app/facts.rs` (`checkout_subjects_using`,
`snapshot_run_git_root`, `git_subjects`), `bridge/src/app/watchers.rs`
(`WorktreeWatchers::reconcile`), `bridge/src/changes.rs` (`Scope::covers`), and
`bridge/src/app/observation_reads/scopes/files.rs` (`ScopeReadContext::file_root`).
Workspace detail exports owner metadata in `app/workspaces/snapshots.rs`, but
this patch does not infer new watcher ownership from it. The proposed real
watcher integration was withdrawn with the run-root design; none is claimed.

A held directory-window refresh now restarts once when an internal
continuation is refused with `invalid_params`. The new head and continuations
retain the original owner predicate and cache-write basis. Cached rows,
selection and preview remain painted while the replacement head is pending.
A second internal refusal stops without persisting a cursor error or replacing
the cache. Each attempt retains the existing eight-page cap: the ceiling of
sixteen reads is **derived from two capped eight-page attempts**, not an
assertion that a fixture executed sixteen reads. The required second-continuation
refusal trace makes six calls and finishes with all 300 held rows cached;
repeated refusal stops after four calls. Hidden/retired owners and a newer
cache write still prevent publication.

Absent-birth-time directory leases now have deadline-driven cleanup, releasing
expired strong file handles even when no further directory request arrives.
One cleanup worker per registry starts lazily for the fallback; birth-time
reads do not start it. Dropping its owner clears held entries, wakes the worker
and joins it. The maximum of 32 held directory handles, absolute 120-second
TTL, oldest-first eviction and explicit refusal behavior remain unchanged.
The new tests check idle release, owner shutdown and the birth-time fast path.

## Frozen-source qualification

The focused JavaScript GREEN exited 0: four files, 43 tests. ESLint exited 0.
Those development runs were made with the patched working tree before its
JavaScript commits; they are not relabeled as final frozen-head runs.
Rust development formatting exited 0, followed by the tree-pages group at
exit 0 with 13 tests, before the Rust source commit. These also remain
development receipts rather than final frozen-head qualification.

The affected selection retains the previous conservative 369 unit files and
11 revision browser files, then includes every exact fixture match found for
the changed subscription/error/cache contracts. This adds
`taskReviewChanges.test.js`, `transientRead.test.js`, and browser
`workspaceLock.test.js`, all containing `invalid_params` fixtures: **371 unit
files and 12 browser files**. The selection manifest retains each search and
its matching paths. Three performance drivers remain separate and unexecuted:
`syncTimingCutover.test.js`, `compat335.test.js`, and `taskOpenCost.test.js`.

All fresh frozen-head positive gates completed at exit 0:

| Gate at full `b38e11bc78` source | Actual result |
| --- | --- |
| Tree pages | 13 tests, exit 0 |
| Filesystem | 69 tests, exit 0 |
| SQL reads | 15 passed, 1 ignored, exit 0 |
| Root-free file hints | 4 tests, exit 0 |
| Rust formatting / Clippy | Both exit 0 |
| SPA lint | Exit 0 |
| SPA affected unit selection | 371 files, 6193 tests, exit 0 |
| SPA revision browser selection | 12 files, 80 tests, exit 0 |
| SPA build | Exit 0 |
| Semgrep | 200 rules, 85 targets, 0 findings, exit 0 |

[Backend results](gate-receipts/review-round-3/final-b38e11bc78/review3-final-backend/result.json)
and [exact commands](gate-receipts/review-round-3/final-b38e11bc78/review3-final-backend/commands.jsonl)
retain those exits. Raw logs use lossless `raw_utf8` JSON storage with original
hashes in the [receipt manifest](gate-receipts/review-round-3/manifest.json).
The [positive master](gate-receipts/review-round-3/final-b38e11bc78/review3-final/result.json)
and [UI result](gate-receipts/review-round-3/final-b38e11bc78/review3-final-ui/result.json)
completed at exit 0. The completed
[unit log](gate-receipts/review-round-3/final-b38e11bc78/review3-final-ui/unit.log.json)
records 174.85 seconds for this targeted selection, not a full SPA assertion.
The [browser log](gate-receipts/review-round-3/final-b38e11bc78/review3-final-ui/layout.log.json)
records 125.98 seconds for the revision selection. Neither result is a new
performance cohort.
| Behavioral mutation at `b38e11bc78` | Actual negative test | Wrapper / restoration |
| --- | --- | --- |
| Remove internal-window restart | Exit 1, all six expected failing names | Expected wrapper 1; restoration verified |
| Restore wrong original-source review spec | Exit 1, zero-read assertion sees two filesystem calls | Wrapper 0; all restore checks 0 |
| Disable idle lease cleanup | Rust runtime exit 101 after successful compilation; named idle-expiry test retains one handle instead of zero | Wrapper 0; all 17 nested restore/source checks 0 |

[Internal restart result](gate-receipts/review-round-3/controls/spa-tree-internal_restart-negative-b38e11bc78fd.result.json)
and [workspace result](gate-receipts/review-round-3/controls/review-workspace-source-control-b38e11bc78/result.json)
retain exact failures, copy/touch/compare commands and restored source hashes.
The [idle control ledger](gate-receipts/review-round-3/controls/directory-lease-idle-no-worker-b38e11bc/argv.jsonl)
retains successful compilation followed by the exact runtime assertion,
`idle timer must release the last retained directory File` (`left: 1`,
`right: 0`). Its 3m38s compile and 2.00s test duration are not read latency.
The [control master](gate-receipts/review-round-3/final-b38e11bc78/review3-controls/result.json)
exited 0 after checking the distinct expected wrappers; its command ledger
also records the final restored source identity.
All three exact mutation inputs are retained separately. A failed negative
test is not described as a passing positive suite.

Development Rust formatting was reported below 0.2 seconds wall time without
absolute timestamps; a possible overlap cannot be established or excluded.
It is functional evidence only. All final gate commands run strictly serially.

## Test-first failures retained

| Development check | Actual result and implication |
| --- | --- |
| Copied workspace original-source hint RED | Exit 1: `ignores original project source hints for a copied workspace directory` failed the zero-read assertion with exactly two calls, `fs.tree(src)` and `fs.read(src/live.txt)`. |
| Internal continuation / retry-boundary RED | Exit 1: the old implementation did not restart a refused held-window continuation. The full named failures remain in the retained development log. |
| Initial idle-expiry test | Exit 101 at compilation because the new cleanup-owner seam did not yet exist. This is a compile RED. The later no-worker mutation compiled successfully and then exposed the named runtime handle-retention failure. |

## Measurement attribution

[Review round 2](review-round-2.md) and its raw receipts remain attributed to
runtime/test source `907ae03e33c2bf39b108d721a3cff72a9ce3a8d6`; its artifact
commit is `ffb8d5992a16f8cf3f08fc223d9df4c0f754c82b`. Those measurements are
historical evidence and do not qualify this round's runtime changes.

The previously observed long-history warm task-open wall/title increase is a
single observation per arm, with all four task-open rows and raw samples
retained. There is no causal claim, retry, or performance action in this round.
Small-task metadata byte overhead remains disclosed separately from the
passing small-startup deterministic no-regression checks. Earlier append and
release-read observations keep their own original source pins. This round's
derived sixteen-read recovery bound is a deterministic limit, not a new
wall-clock or throughput claim.
