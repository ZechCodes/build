# Independent review record

This record concerns design/isolated research handoff only. Production
implementation, activation, cost acceptance, final epic Astra disposition and
later production code review remain outside these approvals.

## Independent Sol scope/source review

Reviewer: separate `/root/scope_audit` Sol agent, read-only at exact fcec6987.
Initial disposition requested three corrections: distinguish assignee `any`
presence from held-kind truthiness; include all non-decoration card fields in
project epoch coverage; clarify invalid-label JSON versus [] fallback.
The design was corrected and independently rechecked. Verdict: **approve
revised design/research scope**, no remaining scope findings. Domain guards,
off-page invalidity, cached roster authority and no production claims were
explicitly reviewed. Final scope recheck also reaffirmed approval after inspecting EVIDENCE and the
configured-column/cache-version corrections. No builds/timing were run by this reviewer.

## Separate Sol architecture review

Reviewer: separate `/root/architecture_review` Sol agent, read-only.
Initial request-changes findings uncovered real prototype/evidence failures:
expression-index cursor prefix scans, coarse callback VM undercount, workspace/
readiness/unfinished/Board oracle errors, and missing service envelope costs.
Design also clarified configured-only Backlog, ordered configuration including
duplicate slots, historical card-version retention and local mutation fences.

Final verdict: **approve for research handoff**, no remaining architecture
blockers in the inspected packet. Reviewer independently checked bdb296735
source hashes, annotation-only AST equality, exactly 32 unique final scenario
identities, progress/fairness failure outcomes, and selected raw counters.
The narrow semgrep SQL annotations were inspected: all structural fragments
come from fixed internal variants and all external values remain bound.
No new builds/timing were run by this reviewer.

The approval explicitly leaves production canonical-projection parity,
complete byte accounting, WAL/disk/lock contention, rollback/recovery, latency,
quotas/concurrency and activation unvalidated. It does not accept those costs
or authorize implementation. #891 consumer review received the concrete packet;
future production dispatch remains separately gated by #887.
