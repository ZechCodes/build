// The agent rail's pure model: who the agents are, what the bubble strip says
// about each of them, which conversation is open, and how a completion report
// reads.
//
// The rail belongs to one work item — a branch or an issue — and the bridge
// answers for that item with one payload (branch.get / issue.get) carrying its
// agents and its conversation. Everything here reads that payload; core/
// agentRail.js renders and wires it.

import { entityIdOf } from "./entityId.js";
import { unreadReasonText } from "./inbox.js";
import { STARTABLE_PROVIDERS } from "./modelPicker.js";

/** The harness's name as a person says it. An unknown provider is shown as the
 *  bridge named it — a new harness must read as itself, not as "Agent". */
export function providerLabel(provider) {
  if (!provider) return "Agent";
  const known = STARTABLE_PROVIDERS.find((entry) => entry.id === provider);
  return known ? known.label : String(provider);
}

/** Which agent this is, in words: the harness and its place on the strip. */
export function agentTitle(agent) {
  return `${providerLabel(agent && agent.provider)} ${(agent && agent.ordinal) || 1}`;
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
 * A work item Build owns no agent in yet gets a single GHOST bubble instead:
 * the conversation exists before the agent does, and the first message is what
 * brings the agent into being. Nothing can be added beside an agent that is not
 * there yet, so the `+` waits for it.
 */
export function railBubbles({ agents = [], selectedId = null, kind = "branch" } = {}) {
  if (!agents.length) {
    return [{ type: "ghost", id: "", label: "1", title: "Send a message to start an agent here", active: true, unread: 0, working: false }];
  }
  const bubbles = agents.map((agent) => ({
    type: "agent",
    id: agent.id,
    label: String(agent.ordinal || 1),
    title: bubbleTip(agent),
    active: agent.id === selectedId,
    unread: agent.unread_count || 0,
    working: !!agent.working,
    live: agent.state === "live",
  }));
  // Issues carry exactly one agent session: implementing one hands the work to
  // a new agent on a branch, which is a different work item entirely.
  if (kind === "branch") {
    bubbles.push({ type: "add", id: "", label: "+", title: "Add another agent to this branch", active: false, unread: 0, working: false });
  }
  return bubbles;
}

/** The conversation that is open: the one the human chose while it still
 *  exists, else the first — the rail is never open on nothing. */
export function selectAgentId(agents = [], wanted = null) {
  if (wanted && agents.some((agent) => agent.id === wanted)) return wanted;
  return agents.length ? agents[0].id : null;
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
