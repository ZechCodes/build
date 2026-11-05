# #894 final review qualification

Runtime source `907ae03e33c2bf39b108d721a3cff72a9ce3a8d6` passes the final positive qualification. Seven deliberate controls produced their expected test failures, with restoration verified. The corrections preserve bounded reads, cached content and reading place; the final source and controlled tests were restored and compared after every mutation. Secret scan/publication are recorded separately by the parent after the artifact commit.

## Final gates and controls

[The final manifest](gate-receipts/review-round-2/final-907ae03e33/manifest.json) retains exact outer/inner command ledgers, actual exits, source proofs and lossless raw logs. Positive stages ran serially; controls ran after the positive master completed.

| Final target | Observed result |
| --- | --- |
| Directory paging / full filesystem | 10 / 69 tests; both processes exit 0 |
| Runtime SQL reads | 15 tests pass, one timing test ignored; process exit 0 |
| Files hints | 4 tests; exit 0 |
| Rust format / all-target Clippy | Both exit 0 |
| SPA lint / selected unit union | Lint 0; 369 files / 6138 tests, exit 0 |
| Revision browser selection / build | 11 files / 79 tests, exit 0; build 0 |
| Matched cost wrapper | Five fixture processes and both comparisons exit 0; complete source restoration verified |
| Semgrep | Exit 0; 85 targets, 200 applicable rules, no findings or timeout warnings |
| Seven control verification | Outer result 0; expected failures and every copy/touch/cmp/source check verified |

| Deliberate defect | Test process | Wrapper / restoration |
| --- | ---: | --- |
| Old timeline merge | 1, five named legacy-upgrade failures | Wrapper 0, restore 0 |
| Old review board/state watcher | 1, unrelated state causes three filesystem reads | Wrapper 0, restore 0 |
| Missing review cursor-notice guard | 1, expired cursor notice becomes visible | Wrapper 0, restore 0 |
| Truncated hint refreshes only one directory | 1, two named visible-directory failures | Wrapper 1 intentionally propagated, restore 0 |
| Refresh discards held pages | 1, selected three-page row disappears | Wrapper 1 intentionally propagated, restore 0 |
| Reveal stops at the first page | 1, requested later-page row is absent | Wrapper 1 intentionally propagated, restore 0 |
| Weak directory handle | Runtime 101 after successful compilation, named cold-provider failure | Wrapper 0, seven restore/source checks 0 |

[Control results](gate-receipts/review-round-2/final-907ae03e33/controls-result.json) preserve those distinct exits. Each per-control result/ledger contains exact failed names; [defective-source storage](gate-receipts/review-round-2/final-907ae03e33/defective-source-storage.json) retains all seven mutation inputs byte-for-byte. The head-specific a455 watcher/cursor copies were prepared but never executed; actual generic runners checked their explicit source argument against HEAD.

The [source-equivalence proof](gate-receipts/review-round-2/final-907ae03e33/source-equivalence.json) shows the bridge, SPA production, fixtures/scripts and all controlled source/test/driver objects equal a455. Only the unrelated `cacheSyncDelivery.test.js` readiness wait changed. Fresh controls nevertheless ran at 907. [The post-controls proof](gate-receipts/review-round-2/final-907ae03e33/post-controls-source-proof.jsonl) records clean runtime/test/script source and restored hashes.

## Corrected bounded behavior

- Partial legacy task heads without their own continuation or matching incarnation take the incoming paged head and cursor. Complete legacy history stays complete. Unit/scoped-cache and mounted Task tests cover offsets 0/3, repeated merge, unfenced cursors and Older history page two.
- Truncated hints refresh every expanded visible directory, including unselected siblings. Folded descendants retain stale cache and issue no reads.
- Refresh rebuilds toward the held dir-first/name key, using the held page count capped at eight pages per directory. Older caches infer `ceil(entries.length/100)`, capped at eight. The replacement window publishes atomically; a tail file becoming a same-name directory does not prematurely end restoration.
- Reveal stops when the requested component is found, with at most eight additional continuation reads shared across ancestors. Beyond those budgets, further loading remains explicit user demand.
- Live review Files reuses `directoryWatchSpec` for exact `project_source` / `files.hints`. Its modern subject-less fallback handles explicit/reconnect refresh, ignoring ordinary state items. The legacy workspace/files branch remains. Captured generation, current visibility and selected-file predicates guard hint reads.
- Review continuation `invalid_params` is left to quiet generic recovery without persisting a notice. Cached rows, selected preview and header remain painted while the replacement head is pending. Genuine first-page `invalid_params` and continuation `unavailable` errors remain visible.
- Missing birth metadata uses real held-directory leases when Unix device/inode identity exists: at most 32 retained handles, absolute 120-second lifetime, oldest-first eviction. Removed/replaced paths and expired/evicted cursors refuse explicitly. Missing platform identity still refuses. Each scan remains O(n) with bounded top-k memory; FS pages default to 100, maximum 200.

The absent-birth-time evidence composes actual Rust directory I/O / registry tests with separately scripted SPA response/refusal contracts. It is not an installed-bridge end-to-end deployment. Secondary workspace sources still lack an independent durable filesystem producer; open/reconnect refresh does not promise replay of lost external filesystem notifications. No supported-version floor was approved for retiring legacy paths.

## Fresh matched performance and compatibility

[The fresh cost manifest](cutover-review-907ae03e33/manifest.json) pins 907 against exact baseline `e1c4b21fc3c16e1ab3a2dd70b155efff8a214c03`. All three drivers retain their frozen hashes. The 18-row startup baseline is the original exact-base measurement; the 3.35 and mounted-task baseline arms were freshly executed under checked production-source replacement. The runner restored all 38 affected files from saved copies with touch/cmp and verified all 1724 non-ignored SPA files, SHA-256 `90e94fea991842c18c23c3342e3d4685596e40c11c15bce533a0684343218f30`.

| SPA production source | Synthetic 3.33 peer | Synthetic 3.34 peer | Synthetic 3.35 peer |
| --- | --- | --- | --- |
| Exact e1c4b21f | Nine legacy startup records | Nine scoped startup records with warmers | Nine records; new paging/hint flags ignored |
| Exact 907ae03e33 | Nine records; deterministic work identical | Nine scoped lazy-start records | Nine records; real adapter recognizes paging/hint flags |

Each cell covers small, single-project and large profiles across cold, warm and long-absent starts. These are synthetic peers exercising real SPA/cache/engine code, not four installed bridge deployments. Bridge wire fixtures, cache upgrade/downgrade and 3.34→3.33→3.34 rollback retain their separate [coverage map](cutover-coverage.md).

The scoped 3.34 startup rows below retain all requested logical/cache/publication counters. All nine legacy 3.33 cells preserve response bytes, RPCs, logical time and IDB callbacks; the full tables include every wall observation. The separate 3.35 adapter fixture has its own matched baseline callback counts and is not mixed with this table.

| Peer | Profile/start | Response bytes | RPCs | Logical ms | IDB callbacks | Cache wall ms | Row publication ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 3.34.0 | small:cold | 61029 → 8681 | 23 → 3 | 4300 → 300 | 580 → 254 | 53.303 → 33.593 | 0 → 0 |
| 3.34.0 | small:warm | 6339 → 4105 | 16 → 2 | 3400 → 200 | 472 → 158 | 43.336 → 13.173 | 0 → 0 |
| 3.34.0 | small:long-absent | 11052 → 8818 | 18 → 4 | 3600 → 400 | 656 → 336 | 51.662 → 30.829 | 0 → 0 |
| 3.34.0 | single-project:cold | 2006126 → 199279 | 247 → 5 | 52450 → 500 | 3844 → 1602 | 233.908 → 132.458 | 200 → 200 |
| 3.34.0 | single-project:warm | 103005 → 92221 | 195 → 2 | 42550 → 200 | 3011 → 1005 | 363.086 → 86.429 | 0 → 0 |
| 3.34.0 | single-project:long-absent | 210200 → 199416 | 199 → 6 | 42950 → 600 | 4490 → 2481 | 646.433 → 231.667 | 200 → 200 |
| 3.34.0 | large:cold | 6693967 → 666757 | 826 → 10 | 175800 → 1000 | 13166 → 5342 | 1062.838 → 342.671 | 700 → 700 |
| 3.34.0 | large:warm | 346367 → 306253 | 650 → 2 | 143000 → 200 | 10338 → 3282 | 3258.087 → 302.915 | 0 → 0 |
| 3.34.0 | large:long-absent | 707008 → 666894 | 659 → 11 | 143900 → 1100 | 15296 → 8216 | 6470.046 → 1286.974 | 700 → 700 |

Small-startup deterministic gates pass without increases. Logical time is the fixed RPC-clock model; IDB callbacks are actual fake-IndexedDB work, and row-publication times in this table are logical. Cache wall observations are not production storage/network latency. Legacy wall samples vary upward despite unchanged deterministic work; no causal latency conclusion follows.

Mounted selected-task cost includes every scope/detail call during mount: one snapshot + four changes + one `tasks.get` cold, and two `tasks.get` calls warm. Device bootstrap is precompleted; push-subscription wire, read marks, reviews, actions, SQL/disk and whole-app startup are excluded.

| History/start | RPCs | Request JSON B | Response JSON B | Combined JSON B | IDB callbacks | Publications |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 20 cold | 6 → 6 | 605 → 638 | 11,676 → 11,788 | 12,281 → 12,426 | 248 → 248 | 2 → 2 |
| 20 warm | 7 → 7 | 665 → 731 | 16,993 → 17,217 | 17,658 → 17,948 | 273 → 273 | 3 → 3 |
| 4096 cold | 6 → 6 | 605 → 638 | 1,000,235 → 26,390 | 1,000,840 → 27,028 | 248 → 248 | 2 → 2 |
| 4096 warm | 7 → 7 | 665 → 731 | 1,986,810 → 39,120 | 1,987,475 → 39,851 | 273 → 273 | 3 → 3 |

The 20-comment task adds **145 combined JSON bytes cold / 290 warm**: 112/224 response metadata plus 33/66 request bytes. This explicit small-task cost is separate from the passing small-startup counter gate. All RPC/cache/publication counts stay equal. JSON totals exclude RPC/encryption framing and transport overhead.

| History/start | DOM-title observation ms | First cache publication ms | Mount wall ms |
| --- | ---: | ---: | ---: |
| 20 cold | 255.542 → 249.635 | 74.840 → 73.904 | 296.800 → 291.475 |
| 20 warm | 84.136 → 84.704 | 138.113 → 137.868 | 184.277 → 181.858 |
| 4096 cold | 217.926 → 214.325 | 55.819 → 56.694 | 264.465 → 288.468 |
| 4096 warm | 145.531 → 436.968 | 203.202 → 504.887 | 271.803 → 946.322 |

**The long-warm observation increases:** mount wall 271.803→946.322 ms and title 145.531→436.968 ms, despite RPC 7→7, IDB callbacks 273→273 and publications 3→3. All four rows remain; no retry or causal improvement/regression claim is made. The title timer is a jsdom MutationObserver, not browser pixels/frames. Both warm arms paint cached titles before held RPC replies are released; cold has no cached paint.

The original 50d startup/mount cohort, release-carrier timings and append samples remain unchanged and source-attributed. The separate endpoint probe's reply JSON 2,528,313→31,933 bytes / one invocation each uses different comment bodies; it is not the whole mounted opener. The append cohort still retains all 87 observations per arm/history and every round median: no measurable wall change at its noise level, with deterministic 44→37 writes, 181→168 executions and 127→122 SELECT starts. No fresh append-wall or release-carrier run is claimed here.

## Failures, resolutions and evidence limits

| Earlier result | Named failure / resolution |
| --- | --- |
| df14 broad unit exit 1: 368 files / 6134 tests passed | `changesReviewPane > workspace comments > anchors selected text with ambient context disabled`: code row not painted. Added an explicit painted-row readiness wait; focused 22 tests passed. Browser/build/cost/Semgrep never started in that failed master. |
| a455 broad unit exit 1: 368 files / 6137 tests passed | `cacheSyncDelivery > a pass racing the wire > does not let the board it read overwrite a row pushed while the lists were out`: assertion ran before orderedSync phases/cache writes completed. Only added read-only `passesSettled` before the unchanged exact agents assertion; no desired-value polling or corrective read. Browser/build/cost/Semgrep never started in that failed master. |
| Review watcher behavioral RED exit 1 | `ignores unrelated board state under root-free hints`: root tree, src tree and selected body were reread. Exact source-scoped hints / quiet fallback fixed it. |
| Review cursor behavioral RED exit 1 | `keeps cached live-review rows and preview quiet during a refused directory continuation`: actual visible expired-cursor notice while replacement head was held. Narrow persistence guard fixed it; ordinary errors remain tested. |
| Development ESLint exit 1 | Review/tree/timeline complexity findings were refactored without raising limits or weakening assertions; final lint is 0. |

The [pre-guard receipts](gate-receipts/review-round-2/manifest.json) and [a455 receipts](gate-receipts/review-round-2/guarded-a455b832cd/manifest.json) preserve original exits and names. The original-round [gate receipts](gate-receipts/status.json) retain its earlier Rust/SPA fixture failures. Original 76-file/597-test browser evidence is prior-round evidence, with the separately pinned 50d lifecycle supplement; it is not relabeled as the fresh 907 run of 11 files/79 tests.

A potential earlier development fmt/Clippy launch overlap is disclosed as a scheduling deviation, not performance evidence. Final positive stages and controls ran serially. The source command pack retains exact available ledgers/script bodies; the inherited disclosure of 14 missing historical command bodies and incomplete early discovery remains. Child taskAPI ledgers stay separate and unchanged.

All raw logs and mutation sources use lossless UTF-8 JSON wrappers with original SHA-256 identities; decoding `raw_utf8` reproduces the exact original bytes. No whitespace suppression, scanner allowlist, full SPA assertion, live watcher integration, installed bridge or deployment is claimed. The coordinator's full SPA gate and secret scan/publication are recorded separately by the parent after the artifact commit.
