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
import { gitStatusCells } from "./gitStatusCells.js";
import { unreadReasonText } from "./inbox.js";
import { providerLabel } from "./modelPicker.js";
import { humanAge } from "./text.js";
import { isStartupEvent, startupEventTitle } from "./threadEvents.js";

// One naming table for the whole client (core/modelPicker.js): the new-agent
// cards, the Account select and the rail's bubbles all say the same word for
// the same harness.
export { providerLabel };

/** Which agent this is, in words: the harness and its place on the strip. */
export function agentTitle(agent) {
  return `${providerLabel(agent && agent.provider)} ${(agent && agent.ordinal) || 1}`;
}

/** What the header over the conversation says: the topic the agent named its
 *  work with (`set_topic`, 2-4 words), or "Starting" until it has — flagged so
 *  the head can shimmer the word rather than sit on it. A blank topic is no
 *  topic. */
export const STARTING_HEADING = "Starting";
export function agentHeading(agent) {
  const topic = agent && typeof agent.topic === "string" ? agent.topic.trim() : "";
  return topic ? { text: topic, starting: false } : { text: STARTING_HEADING, starting: true };
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

/** The state of an agent this client has asked for a session for and not been
 *  told about yet.
 *
 *  The daemon answers a start as soon as the turn is durable and opens the
 *  harness behind that answer, so the reply says nothing about a session. The
 *  row wears this until the entity's own push says the session is live; it is
 *  the client's word, and no payload ever carries it. */
export const AGENT_STARTING = "starting";

/** Whether this agent has a session running right now — the one answer to a
 *  start, and the only one the daemon speaks. */
export const agentSessionIsLive = (agent) => !!agent && agent.state === "live";

/** Why this agent's last start never reached a harness, or "" when the last one
 *  did. The second answer to a start: a spawn that failed says nothing about a
 *  session, so without this the row wears "starting" until the overlay's grace
 *  runs out and then goes quietly idle with the reason nowhere. */
export const agentStartFailure = (agent) => (agent && agent.start_error) || "";

/** Whether the daemon has answered the question a start asks. Two answers, and
 *  the client's own `AGENT_STARTING` stands until one of them arrives: a
 *  session is live, or none ever opened and this is why. */
export const agentSessionAnswered = (agent) => agentSessionIsLive(agent) || !!agentStartFailure(agent);

/** The starts that have just been reported failed: an agent the rail was
 *  already showing, with no reason on it, that now carries one. A reason that
 *  was there before this comparison began is old news — the row says it, and
 *  nothing is raised over it a second time. */
export function startFailuresLearned(before = [], after = []) {
  const known = new Map(before.map((agent) => [agent.id, agentStartFailure(agent)]));
  return after.filter((agent) => {
    const failure = agentStartFailure(agent);
    return !!failure && known.has(agent.id) && known.get(agent.id) !== failure;
  });
}

/** Whether this agent has a session or is getting one. The question the
 *  composer asks before waking an agent, so a message sent behind a start that
 *  has not been answered does not open a second harness. */
export const agentIsUp = (agent) => agentSessionIsLive(agent) || (!!agent && agent.state === AGENT_STARTING);

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
  if (agent && agent.state === AGENT_STARTING) return `${title} — starting…`;
  const failure = agentStartFailure(agent);
  if (failure) return `${title} — ${failure}`;
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
const supportsMultipleAgents = (kind) => kind === "branch" || kind === "workspace";
const addAgentTitle = (kind) => `Add another agent to this ${kind === "workspace" ? "workspace" : "branch"}`;

export function railBubbles({ agents = [], selectedId = null, kind = "branch", chatCapable = true, addingAgent = false } = {}) {
  if (!agents.length) {
    return [
      {
        type: "ghost",
        id: "",
        label: "",
        pattern: agentPattern(1),
        title: chatCapable ? "Send a message to start an agent here" : "No agent conversation is attached to this workspace",
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
    active: !addingAgent && agent.id === selectedId,
    unread: agent.unread_count || 0,
    working: !!agent.working,
    live: agentSessionIsLive(agent),
    starting: agent.state === AGENT_STARTING,
  }));
  // Issues carry exactly one agent session: implementing one hands the work to
  // a new agent on a branch, which is a different work item entirely.
  if (supportsMultipleAgents(kind)) {
    bubbles.push({
      type: "add",
      id: "",
      label: "+",
      pattern: null,
      title: addAgentTitle(kind),
      active: addingAgent,
      unread: 0,
      working: false,
    });
  }
  return bubbles;
}

/**
 * Whether this agent can be taken back off the work item — the mirror of the
 * `+` bubble, and it answers the same question the daemon does: is this a
 * branch, and does the id name an agent on it?
 *
 * Branches only: an issue's one agent IS the issue's conversation, so there is
 * nothing to remove there, only an issue to abandon. Every agent on a branch
 * may go, the first and the last included — a branch left with none is a
 * working branch whose chat tab asks which agent to start one on.
 */
export function canRemoveAgent({ agents = [], agentId = null, kind = "branch" } = {}) {
  if ((kind !== "branch" && kind !== "workspace") || !agentId) return false;
  return agents.some((agent) => agent.id === agentId);
}

/** The confirmation plan for `agent.remove` — the outline core/confirm.js asks
 *  with. Removal kills the agent's session and takes its conversation with it,
 *  so it says both, and says what it does NOT touch. */
export function removeAgentConfirm(agent, kind = "branch") {
  const who = agentTitle(agent);
  const owner = kind === "workspace" ? "workspace" : "branch";
  return {
    title: `Remove ${who} from this ${owner}?`,
    actions: [
      "End the agent's session, if one is running",
      `Remove ${who} and its conversation from the ${owner}`,
      `Leave the ${owner} and its files untouched`,
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

/** The elapsed-time clock the pinned line ticks: seconds alone under a
 *  minute, minutes and seconds under an hour, hours and minutes beyond —
 *  never days, which a line this narrow has no room to read. */
const CLOCK_TURNS_TO_HOURS_AT_SECONDS = 90 * 60;

const twoDigits = (value) => String(value).padStart(2, "0");

export function workingClock(seconds) {
  const elapsed = Math.max(0, Math.floor(seconds || 0));
  if (elapsed < 60) return `${elapsed}s`;
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ${twoDigits(elapsed % 60)}s`;
  return `${Math.floor(elapsed / 3600)}h ${twoDigits(Math.floor((elapsed % 3600) / 60))}m`;
}

export function runningClock(seconds) {
  const elapsed = Math.max(0, Math.floor(seconds || 0));
  if (elapsed < CLOCK_TURNS_TO_HOURS_AT_SECONDS) {
    return `${Math.floor(elapsed / 60)}:${twoDigits(elapsed % 60)}`;
  }
  return `${Math.floor(elapsed / 3600)}:${twoDigits(Math.floor((elapsed % 3600) / 60))}`;
}

export function elapsedClock(sinceMs, nowMs) {
  return runningClock((nowMs - sinceMs) / 1000);
}

export function startupStatusLine(conversationItems = [], agentLabel = "Agent") {
  const newest = conversationItems[conversationItems.length - 1];
  if (!isStartupEvent(newest)) return null;
  const event = newest.data || {};
  const at = Date.parse(event.created_at || "");
  return { title: startupEventTitle(event, agentLabel), at: Number.isFinite(at) ? at : null };
}

function startupText(startup, nowMs) {
  if (!startup) return "";
  if (startup.at === null) return startup.title;
  return `${startup.title} · ${humanAge((nowMs - startup.at) / 1000)}`;
}

/** The pinned line above the composer: whether the work item has a turn in
 *  flight right now (and for how long), how far it stands from upstream, and
 *  its diffstat. The clocks are "" and the git facts an empty list when the row
 *  does not know them, so a work item with nothing to report pins nothing at
 *  all. The git facts are characters rather than text because the line moves
 *  one character at a time — see core/gitStatusCells.js. */
export function railWorkStatus(row, nowMs = Date.now(), conversationItems = [], agentLabel = "Agent", agent = null) {
  // A branch/issue row may still carry the old entity aggregate. It is not an
  // agent timer: parallel agents have independent turns, so only the selected
  // agent's durable field may drive this clock.
  const working = workingSeconds(agent && agent.working_time, nowMs);
  return {
    working: working === null ? "" : runningClock(working),
    starting: working === null ? startupText(startupStatusLine(conversationItems, agentLabel), nowMs) : "",
    git: gitStatusCells(row && row.stat),
  };
}

export const WORKING_SHAPE = "working";
export const STARTING_SHAPE = "starting";
export const QUIET_SHAPE = "quiet";

export function railStatusShape(status) {
  if (status.working) return WORKING_SHAPE;
  if (status.starting) return STARTING_SHAPE;
  return QUIET_SHAPE;
}

/**
 * What the rail is the rail OF, read off one detail payload.
 *
 * A branch row names its entity through the same helper the inbox uses
 * (core/entityId.js), and carries its run's conversation when Build owns one.
 * A branch with no run is a checkout nobody has claimed: the rail still shows a
 * conversation, and sending in it is what adopts the checkout.
 */
// eslint-disable-next-line complexity -- ratchet: railEntity is at 19, cap 10 — reduce it, then drop this line
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
      executionContext: row.execution_context || null,
      agents: row.agents || [],
      thread: row.thread || null,
    };
  }
  const agents = row.agents || (row.run && row.run.agents) || [];
  if (kind === "workspace") {
    const entityId = row.entity_id || row.run_id || (row.run && row.run.id) || null;
    return {
      entityId,
      kind: "workspace",
      projectId: row.project_id || null,
      branch: row.branch || (row.directories || []).find((directory) => directory.branch)?.branch || null,
      worktreeId: null,
      primary: false,
      adoptable: false,
      canAdd: !!entityId && agents.length > 0,
      chatCapable: true,
      executionContext: row.execution_context || null,
      agents,
      thread: (row.run && row.run.thread) || row.thread || null,
    };
  }
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
    chatCapable: true,
    executionContext: row.execution_context || null,
    agents,
    thread: (row.run && row.run.thread) || null,
  };
}
