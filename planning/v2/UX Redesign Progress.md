# UX Redesign Progress

Session-independent state of the 2026-08 UX redesign build. If the driving
session dies (the bridge daemon restarting kills it), a fresh session resumes
from here. Companion doc: `UX Redesign Decisions.md` (the binding design).

## Do issues (Build project → Planning list, production getdo.ing)

| Issue | Do id | Status |
|---|---|---|
| UX Architecture: Inbox and Control Plane | 01KZVK8Q2TQ2H9THHNJBV8GKTJ | done (validated, marked in Do) |
| Phase 1: Conversation Threads | 01KY5NK17WBNW2J8DVC0239F39 | done (validated, marked in Do) |
| Bridge wedge (2026-08-13 incident) | 01KZYZYMK4GQVHBWXY0C364K60 | done (validated, marked in Do) |
| Capture and Router Agent | 01KZVK8Q2TQ2H9THHNJBV8GKTK | done (validated, marked in Do) |
| Phase 2: Review Prioritization | 01KY5NKASCR6R5JQCYPCJZZMMS | done (validated, marked in Do) |
| Phase 3: Implementation Graphs | 01KY5NKNN1TMQB808EEQMABD66 | skipped — reviewer decision 2026-08-14: the redesign is UX-scoped; graphs are new capability, left open in Do |

Marking off = `fields.status = {type: text, value: done}` via Do's
`update_document_fields` service, run inside the `do-worker` pod
(namespace `<namespace>`); read access likewise via `kubectl exec` +
asyncpg against the `getdo` schema. The Do MCP is not connected.

## Stage log (all on branch build/ui-rebuild)

- Stage A — bridge foundations: attention/status event classes, per-agent
  read cursors + event-driven unread, first-class agents with per-agent
  threads and PTYs, branch rows on board.list, mute, completion reports,
  search_conversation, run→issue mirroring, doc comments as posts.
  Wire contract frozen (also in the Stage B prompts).
- Stage B — SPA shell, built as two parallel worktree tracks
  (b-shell / b-surfaces, both merged and still on disk): three-panel layout,
  #/inbox landing, global inbox rail, toolbar with unified create menu
  (inert issues — no agent until first message), agent rail with bubble
  strip + Chat/TUI toggle, console panel, Changes rewrite (commit-all,
  noise group, comment everywhere), issue view stages|viewer with
  assignment targeting.
- Stage B.5 — gap close (per-agent thread selection on the wire, implement
  worktree targeting, account archive page, single adopter) + live-bridge
  E2E (passed at HEAD; five real-stack fixes) + validation of the first
  two issues.
- Bridge wedge — dispatch off the relay read loop (bounded workers, ordered
  per-terminal lanes), patch-free run_stat, stale-while-revalidate diff
  caches, per-session request coalescing, SPA waiter rejection. Validated.
- Stage C — captures store, branch.dispatch, router session kind + scoped
  MCP tool surface, global compose + advanced panel, decision-rule test
  suite. Live E2E passed (real claude + codex routers). Validated.
- Stage D — Phase 2 triage: cross-language hunk ids, triage phase turn
  seeded by completion reports, persisted results with staleness/re-triage,
  overrides + .build/review-rules.json seed, Changes overlay (criticals
  first, collapsed rationale groups, trust dial), plus Stage C polish.
  Live E2E passed with a real agent producing a triaged diff. Validated.

## Follow-ups surfaced by the E2E passes (not blocking, unfiled)

- Harness prompt delivery is timing-calibrated (paste-wait constants);
  the durable fix is delivery-by-observation (wait for the paste echo).
- Router sessions are not reaped when a codex TUI wedges; plus a harness
  startup race. Both pre-existing infra, exposed during Stage D E2E.
- The router cannot see plain git branches (list_work is board-only), so a
  capture naming an un-checked-out branch routes to an issue.
- Overrides are offered only on critical/low hunks the pass classified;
  normal-level and commit-changeset hunks show no control.
- Second agent's own provider/model ignored on cold spawn (entity's choice
  used); reachable since request_changes can address agent 2.
- board.list legacy keys still ship (SPA still reads a few); coordinated
  removal pending. Cross-session request coalescing likewise.
- Stage E: SKIPPED. Phase 3 graphs judged new capability rather than UX;
  dropped from this effort by the reviewer. Its Do issue stays open and the
  Decisions doc's graph section is design-only, unbuilt.

## Standing constraints

- The launchd bridge (ing.getbuild.bridge) still runs the Aug 9 binary and
  its done socket was unlinked on 2026-08-14 (~01:58) — real agents' done
  reports fail against it and the driving session cannot post to its
  reviewer thread. Fix: `launchctl kickstart -k gui/$(id -u)/ing.getbuild.bridge`
  (binary already installed at ~/.cargo/bin/build-bridge). DO NOT run this
  while a session that matters is live — it kills every session the daemon
  hosts, including the one driving this build. It is the LAST step.
- Workflow model policy: Opus implements, Sonnet verifies (single task,
  step-by-step), Fable escalates/checkins/E2E. Scripts + full agent results:
  the driving session's directory under `~/.claude/projects/`.
- Issue docs snapshot: the session scratchpad (`issue-*.md`); re-pull from Do
  if the scratchpad is gone.
