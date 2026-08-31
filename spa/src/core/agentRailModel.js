// The agent rail's pure model: who the agents are, what the bubble strip says
// about each of them, which conversation is open, and how a completion report
// reads.
//
// The rail belongs to one work item — a branch or an issue — and the bridge
// answers for that item with one payload (branch.get / issue.get) carrying its
// agents and its conversation. Everything here reads that payload; core/
// agentRail.js renders and wires it.
//
// The pinned status line above the composer reads a second source: the same
// shared feed row (board.list) the inbox and the toolbar's jump menu read,
// matched to this work item by core/toolbarModel.js's `toolbarIdentity`. That
// row is where "is an agent working, for how long, and how does the branch
// stand against upstream" actually live — the per-agent payload only carries
// a working boolean, with no stamp to clock it by.

import { entityIdOf } from "./entityId.js";
import { unreadReasonText } from "./inbox.js";
import { providerLabel } from "./modelPicker.js";

// One naming table for the whole client (core/modelPicker.js): the new-agent
// cards, the Account select and the rail's bubbles all say the same word for
// the same harness.
export { providerLabel };

/** Which agent this is, in words: the harness and its place on the strip. */
export function agentTitle(agent) {
  return `${providerLabel(agent && agent.provider)} ${(agent && agent.ordinal) || 1}`;
}

/** Whether this agent has a terminal to drop into.
 *
 *  The terminal is a capability, not a guarantee: a harness that reports its own
 *  reasoning and tool calls is not opaque, so it offers no basement. An older
 *  bridge does not mention the field at all, and silence is not a refusal —
 *  every agent had a terminal before the question could be asked — so only an
 *  explicit `false` takes it away. */
export function agentHasTerminal(agent) {
  return !agent || agent.has_terminal !== false;
}

/** Whether the turn this agent is running can be stopped and re-steered right
 *  now — which is two things at once, and the composer offers the interrupting
 *  send only where both hold: there is a turn in flight, and the child running
 *  it announced the interrupt at startup.
 *
 *  Absent is NO, the opposite of the terminal above: every carrier had a
 *  terminal before the question could be asked, while an interrupt is a
 *  capability a child announces — so an older bridge, and a CLI built before
 *  the feature landed, both leave the plain Send standing.
 *
 *  Headlessness is not asked separately. Only a carrier with no terminal can
 *  answer this true today, and if one with a terminal ever could, the control
 *  belongs there too. */
export function agentCanInterrupt(agent) {
  return !!(agent && agent.working) && agent.can_interrupt === true;
}

/** How many faces there are for an agent to wear. Each is one of the five
 *  tilings in core/tilings.js, painted into the bubble's canvas by
 *  core/agentCanvas.js — moving while the agent works, frozen on the frame it
 *  stopped at when it does not. */
export const AGENT_PATTERN_COUNT = 5;

/** Which face this agent wears, from its place on the work item: the agents
 *  beside each other are the ones that have to be tellable apart, so the strip's
 *  first five are always five different patterns, and the same agent wears the
 *  same one every time the rail is painted. */
export function agentPattern(ordinal) {
  const place = Math.max(1, Number(ordinal) || 1);
  return ((place - 1) % AGENT_PATTERN_COUNT) + 1;
}

/** The bubble's tooltip: who it is, and the one thing it is waiting on. Unread
 *  wins over working — an agent that asked something while it kept going is
 *  still asking. */
export function bubbleTip(agent) {
  const title = agentTitle(agent);
  if (agent && agent.unread_count) {
    const reason = unreadReasonText(agent.unread_reason, "agent");
    return reason ? `${title} — ${reason}` : `${title} — ${agent.unread_count} unread`;
  }
  if (agent && agent.working) return `${title} — working`;
  return title;
}

/**
 * The strip, top to bottom: one bubble per agent, then the `+` that gives a
 * branch another one.
 *
 * An agent's bubble carries a PATTERN, not a number: `label` is empty for it and
 * `pattern` says which face it wears. The `+` is a control rather than an agent,
 * so it keeps its glyph and wears no pattern.
 *
 * A work item Build owns no agent in yet gets a single GHOST bubble instead:
 * the conversation exists before the agent does, and the first message is what
 * brings the agent into being — so the ghost wears the face that first agent
 * will. Nothing can be added beside an agent that is not there yet, so the `+`
 * waits for it.
 */
export function railBubbles({ agents = [], selectedId = null, kind = "branch" } = {}) {
  if (!agents.length) {
    return [
      {
        type: "ghost",
        id: "",
        label: "",
        pattern: agentPattern(1),
        title: "Send a message to start an agent here",
        active: true,
        unread: 0,
        working: false,
      },
    ];
  }
  const bubbles = agents.map((agent) => ({
    type: "agent",
    id: agent.id,
    label: "",
    pattern: agentPattern(agent.ordinal),
    title: bubbleTip(agent),
    active: agent.id === selectedId,
    unread: agent.unread_count || 0,
    working: !!agent.working,
    live: agent.state === "live",
  }));
  // Issues carry exactly one agent session: implementing one hands the work to
  // a new agent on a branch, which is a different work item entirely.
  if (kind === "branch") {
    bubbles.push({
      type: "add",
      id: "",
      label: "+",
      pattern: null,
      title: "Add another agent to this branch",
      active: false,
      unread: 0,
      working: false,
    });
  }
  return bubbles;
}

/**
 * Whether this agent can be taken back off the work item — the mirror of the
 * `+` bubble, and it answers the same two questions the daemon does.
 *
 * Branches only: an issue's one agent IS the issue's conversation, so there is
 * nothing to remove there, only an issue to abandon. And never the branch's
 * FIRST agent, which owns the branch's conversation — that is also what keeps a
 * branch from ever being left with no agent at all.
 */
export function canRemoveAgent({ agents = [], agentId = null, kind = "branch" } = {}) {
  if (kind !== "branch" || !agentId || agents.length < 2) return false;
  if (agents[0].id === agentId) return false;
  return agents.some((agent) => agent.id === agentId);
}

/** The confirmation plan for `agent.remove` — the outline core/confirm.js asks
 *  with. Removal kills the agent's session and takes its conversation with it,
 *  so it says both, and says what it does NOT touch. */
export function removeAgentConfirm(agent) {
  const who = agentTitle(agent);
  return {
    title: `Remove ${who} from this branch?`,
    actions: [
      "End the agent's session, if one is running",
      `Remove ${who} and its conversation from the branch`,
      "Leave the branch and its files untouched",
    ],
    confirmLabel: "Remove agent",
    danger: true,
  };
}

/** The conversation that is open: the one the human chose while it still
 *  exists, else the first — the rail is never open on nothing. */
export function selectAgentId(agents = [], wanted = null) {
  if (wanted && agents.some((agent) => agent.id === wanted)) return wanted;
  return agents.length ? agents[0].id : null;
}

// ---- the pinned status line -------------------------------------------------

/** How long the turn in flight has been running, from the stamp when it parses
 *  (so a line left on screen ticks) and from the bridge's own count when it
 *  does not. null when nothing is working. */
export function workingSeconds(workingTime, nowMs) {
  if (!workingTime) return null;
  const started = Date.parse(workingTime.since || "");
  if (Number.isFinite(started)) return (nowMs - started) / 1000;
  return Number.isFinite(workingTime.seconds) ? workingTime.seconds : null;
}

/** The diffstat as the status line says it: additions and deletions, nothing
 *  else. A row that predates the object shape sends the string ready-made. */
export function statText(stat) {
  if (!stat) return "";
  if (typeof stat === "string") return stat;
  const insertions = stat.insertions || 0;
  const deletions = stat.deletions || 0;
  if (!insertions && !deletions) return "";
  return `+${insertions} −${deletions}`;
}

/** How far the branch stands from its upstream, in the inbox's own glyphs —
 *  "" when there is nothing to report, so an even branch (or an issue, which
 *  has no upstream) says nothing. */
export function aheadBehindText(stat) {
  if (!stat) return "";
  const parts = [];
  if (stat.ahead) parts.push(`↑${stat.ahead}`);
  if (stat.behind) parts.push(`↓${stat.behind}`);
  return parts.join(" ");
}

/** The elapsed-time clock the pinned line ticks: seconds alone under a
 *  minute, minutes and seconds under an hour, hours and minutes beyond —
 *  never days, which a line this narrow has no room to read. */
export function workingClock(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  return `${hours}h ${String(minutes).padStart(2, "0")}m`;
}

/** The pinned line above the composer: whether the work item has a turn in
 *  flight right now (and for how long), how far it stands from upstream, and
 *  its diffstat — each "" when the row does not know it, so a work item with
 *  nothing to report pins nothing at all. */
export function railWorkStatus(row, nowMs = Date.now()) {
  const working = workingSeconds(row && row.working_time, nowMs);
  return {
    working: working === null ? "" : workingClock(working),
    sync: aheadBehindText(row && row.stat),
    stat: statText(row && row.stat),
  };
}

/**
 * What the rail is the rail OF, read off one detail payload.
 *
 * A branch row names its entity through the same helper the inbox uses
 * (core/entityId.js), and carries its run's conversation when Build owns one.
 * A branch with no run is a checkout nobody has claimed: the rail still shows a
 * conversation, and sending in it is what adopts the checkout.
 */
export function railEntity(payload, kind = "branch") {
  const row = payload || {};
  if (kind === "issue") {
    return {
      entityId: row.issue_id || row.plan_id || null,
      kind: "issue",
      projectId: row.project_id || null,
      branch: null,
      worktreeId: null,
      primary: false,
      adoptable: false,
      canAdd: false,
      agents: row.agents || [],
      thread: row.thread || null,
    };
  }
  const agents = row.agents || (row.run && row.run.agents) || [];
  return {
    entityId: entityIdOf(payload ? { ...row, kind: "branch" } : null),
    kind: "branch",
    projectId: row.project_id || null,
    branch: row.branch || null,
    worktreeId: row.worktree_id || null,
    primary: !!row.primary,
    // No run behind the branch means no owner for an agent to report `done` to:
    // the first message adopts the checkout on its way to being sent.
    adoptable: !row.run_id,
    canAdd: !!row.run_id && agents.length > 0,
    agents,
    thread: (row.run && row.run.thread) || null,
  };
}

// The report's four lists, in the order a reviewer reads them: what carries the
// change, what it decided, what it might break, what it left alone.
const REPORT_SECTIONS = [
  { key: "critical_files", title: "Critical files" },
  { key: "decisions", title: "Decisions" },
  { key: "risk_notes", title: "Risks" },
  { key: "skips", title: "Skipped" },
];

/** The report as sections worth rendering. A list the agent left out is left
 *  out here too — an empty heading says nothing and costs a reader a line. */
export function completionReportSections(report) {
  if (!report) return [];
  return REPORT_SECTIONS.map((section) => ({
    title: section.title,
    items: (report[section.key] || []).filter((entry) => String(entry || "").trim()),
  })).filter((section) => section.items.length);
}
