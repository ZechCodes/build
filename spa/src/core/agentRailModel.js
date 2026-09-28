// The agent rail's pure model: who the agents are, what the bubble strip says
// about each of them, which conversation is open, and how a completion report
// reads.
//
// The rail belongs to one work item — a branch or a task — and the bridge
// answers for that item with one payload (branch.get / task.get) carrying its
// agents and its conversation. Everything here reads that payload; core/
// agentRail.js renders and wires it.
//
// The pinned status line above the composer reads a second source: the same
// shared feed row (board.list) the inbox and the toolbar's jump menu read,
// matched to this work item by core/toolbarModel.js's `toolbarIdentity`. That
// row is where "is an agent working, for how long, and how does the branch
// stand against upstream" actually live — the per-agent payload only carries
// a working boolean, with no stamp to clock it by.

import { agentInitials, agentName } from "./agentName.js";
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

/** What the header over the conversation says: the topic the agent named its
 *  work with (`set_topic`, 2-4 words), or "Starting" until it has — flagged so
 *  the head can shimmer the word rather than sit on it. A blank topic is no
 *  topic. */
export const STARTING_HEADING = "Starting";
export function agentHeading(agent) {
  const topic = agent && typeof agent.topic === "string" ? agent.topic.trim() : "";
  return topic ? { text: topic, starting: false } : { text: STARTING_HEADING, starting: true };
}

/** Which agent this is, in words — its NAME when it has one, else the topic it
 *  named its work with, and its harness while it has neither.
 *
 *  The name comes first because it is the one of the three that is about the
 *  agent rather than about what the agent is doing: a topic moves with the
 *  work and a harness is shared by every agent running on it.
 *
 *  The ordinal stays deliberately absent. It is the agent's place on the
 *  strip, and the strip already says it in the face the bubble wears. */
export function agentWho(agent) {
  const name = agentName(agent);
  if (name) return name;
  const heading = agentHeading(agent);
  return heading.starting ? providerLabel(agent && agent.provider) : heading.text;
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

/** The attention kinds that mean something went wrong rather than something
 *  is waiting: an agent wears them even over work in flight. */
const FAILED_REASONS = new Set(["run_failed", "stage_failed", "recovery_failed"]);
export const isFailedReason = (reason) => FAILED_REASONS.has(reason);

/** Whether an agent's unread says what it is doing now. A failure always does;
 *  any other reason is from before the run in flight, so working outranks it —
 *  an agent that finished and was handed more is working, not finished (#201). */
const unreadIsNews = (agent) => !!agent.unread_count && (!agent.working || isFailedReason(agent.unread_reason));

/** The one thing this agent's bubble is waiting on, or "" when it waits on
 *  nothing: a failure, then work in flight, then the unread it left. */
function bubbleNews(agent, heading) {
  if (agent && unreadIsNews(agent)) {
    return unreadReasonText(agent.unread_reason, "agent") || `${agent.unread_count} unread`;
  }
  if (agent && agent.working) return "working";
  // A bubble whose agent has not named its work already opens on "Starting":
  // appending the session's own "starting…" behind it says the word twice and
  // tells the hover nothing the first one did not.
  if (agent && agent.state === AGENT_STARTING) return heading.starting ? "" : "starting…";
  return agentStartFailure(agent);
}

/** The bubble's tooltip: the topic the agent named its work with, and the one
 *  thing it is waiting on. Until there is a topic the tip leads with the
 *  harness behind the shimmering word — two agents on one work item may run the
 *  same harness, and it is their painted faces that tell them apart. */
export function bubbleTip(agent) {
  const heading = agentHeading(agent);
  const said = heading.starting ? `${STARTING_HEADING} · ${providerLabel(agent && agent.provider)}` : heading.text;
  // A named agent leads with its name and keeps what it is doing after it:
  // the reader hovering a bubble is asking which agent this is first.
  const name = agentName(agent);
  const title = name ? `${name} · ${said}` : said;
  const news = bubbleNews(agent, heading);
  return news ? `${title} — ${news}` : title;
}

const supportsMultipleAgents = (kind) => kind === "branch" || kind === "workspace";
const addAgentTitle = (kind) => `Add another agent to this ${kind === "workspace" ? "workspace" : "branch"}`;

/** Whether the strip offers another agent here. The work item answers it when
 *  it has been read — `canAdd` off its own payload — and the kind answers for a
 *  caller holding no payload: a task carries exactly one agent session, so it
 *  never offers. */
const offersAnotherAgent = (kind, canAdd) => (canAdd === null ? supportsMultipleAgents(kind) : !!canAdd);

/** The line under the project's agent: everything below it belongs to the work
 *  item the page is standing on. Keyed like any other entry — the same shape
 *  core/thread.js rules its unread line with — so the reconciler draws it once
 *  and never again. */
export const RAIL_SEPARATOR_ENTRY = Object.freeze({
  type: "separator",
  id: "",
  label: "",
  pattern: null,
  title: "",
  active: false,
  unread: 0,
  working: false,
});

/** The project's mark: its initial, because the project agent is the project's
 *  and a face off the same five tilings would read as one more agent on this
 *  work item. */
export const projectInitial = (name) => (String(name || "").trim().slice(0, 1) || "?").toUpperCase();

/** What the project's bubble is waiting on, in the same order an agent's is
 *  (`bubbleNews`): a failure, then work in flight, then the unread left. */
function projectAgentNews(agents) {
  const unread = agents.reduce((total, agent) => total + (agent.unread_count || 0), 0);
  const asking = agents.find(unreadIsNews);
  if (asking) return unreadReasonText(asking.unread_reason, "agent") || `${unread} unread`;
  return agents.some((agent) => agent.working) ? "working" : "";
}

/**
 * The project's own agent, as a workspace's strip carries it.
 *
 * It is there whether or not the project has a conversation yet: the agent is
 * the project's, not this workspace's, and one nobody has started is still the
 * one every workspace in the project talks to. Until an agent has been born on
 * it the tip says how to start it, the way a ghost's does, rather than reading
 * as a bubble with nothing behind it.
 *
 * It is marked the way an unwatched agent's bubble is (#105) while the
 * project's agent open under it is one the reader does not watch, or, with
 * none open, while they watch none of them; `projectAgentOnTheStrip` says when
 * the strip carries it at all.
 */
export function projectAgentBubble({ name = "", entityId = null, agents = [], active = false, openAgentId = null } = {}) {
  const news = projectAgentNews(agents);
  const who = `Project agent for ${name}`;
  const unwatched = projectAgentMarked(agents, active, openAgentId);
  const title = agents.length ? [who, news].filter(Boolean).join(" — ") : `${who}: send a message to start it`;
  return {
    type: "project",
    id: entityId || "",
    label: projectInitial(name),
    pattern: null,
    title: unwatched ? `${title} · ${UNWATCHED_TIP}` : title,
    unwatched,
    active,
    unread: agents.reduce((total, agent) => total + (agent.unread_count || 0), 0),
    working: agents.some((agent) => !!agent.working),
  };
}

/**
 * The strip, top to bottom: the project's agent and the line under it where the
 * view asked for one, then one bubble per agent, then the `+` that gives a
 * branch another one.
 *
 * An agent's bubble carries a PATTERN, not a number: `label` is empty for it and
 * `pattern` says which face it wears. The `+` is a control rather than an agent,
 * so it keeps its glyph and wears no pattern, and the project's agent wears the
 * project's initial — it belongs to the project, not to this work item, and the
 * strip has to say so at a glance.
 *
 * The half below the line answers for its own `+`: `agents`, `kind` and
 * `canAdd` are the WORK ITEM's, whichever conversation the rail is standing on.
 * A workspace read while the panel is on the project's conversation can still
 * take another agent, and the control that adds one belongs to it.
 *
 * `selectedKind` is which of the rail's one selection is open (#148): the
 * conversation `selectedId` names, the `+`'s chooser, or the chat overview,
 * whose control the rail puts after the `+`. At most one bubble is active.
 *
 * A work item Build owns no agent in yet gets a single GHOST bubble instead:
 * the conversation exists before the agent does, and the first message is what
 * brings the agent into being — so the ghost wears the face that first agent
 * will. Nothing can be added beside an agent that is not there yet, so the `+`
 * waits for it.
 *
 * On the project's OWN page the rail stands on the project's conversation with
 * nothing above the line, and its agents are the project's agent: they wear the
 * project's initial and the squared-off bubble, the same face that conversation
 * has on every workspace's strip, so the page and the strip agree about whose
 * agent this is. `projectName` is what that initial is cut from.
 */
export function railBubbles({ agents = [], selectedId = null, selectedKind = "agent", kind = "branch", chatCapable = true, canAdd = null, projectAgent = null, projectName } = {}) {
  const own = ownBubbles({ agents, selectedId, selectedKind, kind, chatCapable, canAdd, projectName });
  return projectAgent ? underTheProject(own, projectAgent) : own;
}

/** The face a bubble on the project's own conversation wears: the project's
 *  initial in place of a pattern, flagged so the strip squares it off. */
const projectFace = (projectName) => ({ label: projectInitial(projectName), pattern: null, project: true });

/** The work item's half of the strip, with the project's above it: the
 *  project's bubble, the line, and the work item's own below. The panel holds
 *  one conversation, so while it is the project's nothing below the line is the
 *  open one. */
function underTheProject(own, projectAgent) {
  if (!projectAgentOnTheStrip(projectAgent)) return own;
  const beside = projectAgent.active ? own.map((bubble) => ({ ...bubble, active: false })) : own;
  return [projectAgentBubble(projectAgent), RAIL_SEPARATOR_ENTRY, ...beside];
}

/** Whether the reader watches the project's agent: one of its agents is
 *  watched, or none has been born yet to be unwatched. */
const projectAgentWatched = (agents = []) => !agents.length || agents.some(agentIsWatched);

/** Whether a workspace's strip carries the project's bubble, and the line
 *  under it (#105): while the reader watches the project's agent, and an
 *  unwatched one only while its conversation is the one open. */
/** Whether the project's bubble wears the unwatched mark: the open agent's
 *  watch where one of the project's agents is open, else the project's. */
function projectAgentMarked(agents, active, openAgentId) {
  const open = active ? agents.find((agent) => agent.id === openAgentId) : null;
  return open ? !agentIsWatched(open) : !projectAgentWatched(agents);
}

export const projectAgentOnTheStrip = ({ agents = [], active = false } = {}) => active || projectAgentWatched(agents);

/** Whether the reader watches this agent. A bridge that says nothing about
 *  watching leaves `watched` unset, and every agent of it counts as watched. */
export const agentIsWatched = (agent) => agent?.watched !== false;

/** Whether an agent has a bubble on the strip (#105): the ones the reader
 *  watches, and an unwatched one only while its conversation is the one open.
 *  Leaving it — for another agent, the overview or the `+` — takes it off. */
const onTheStrip = (agent, selectedId, selectedKind) =>
  agentIsWatched(agent) || (selectedKind === "agent" && agent.id === selectedId);

const UNWATCHED_TIP = "Not watching";

/** The bubbles of the work item the rail is standing on, and nothing else. */
function ownBubbles({ agents, selectedId, selectedKind, kind, chatCapable, canAdd, projectName }) {
  const onProjectPage = kind === "project";
  const face = (pattern) => (onProjectPage ? projectFace(projectName) : { label: "", pattern });
  if (!agents.length) {
    return [
      {
        type: "ghost",
        id: "",
        ...face(agentPattern(1)),
        title: chatCapable ? "Send a message to start an agent here" : "No agent conversation is attached to this workspace",
        active: selectedKind === "agent",
        unread: 0,
        working: false,
      },
    ];
  }
  const bubbles = agents.filter((agent) => onTheStrip(agent, selectedId, selectedKind)).map((agent) => ({
    type: "agent",
    id: agent.id,
    ...face(agentPattern(agent.ordinal)),
    // A named agent wears its initials; an unnamed one keeps the painted
    // pattern, which says as much as a letter cut from an ordinal would.
    initials: agentInitials(agent),
    title: agentIsWatched(agent) ? bubbleTip(agent) : `${bubbleTip(agent)} · ${UNWATCHED_TIP}`,
    unwatched: !agentIsWatched(agent),
    active: selectedKind === "agent" && agent.id === selectedId,
    unread: agent.unread_count || 0,
    working: !!agent.working,
    live: agentSessionIsLive(agent),
    starting: agent.state === AGENT_STARTING,
  }));
  // Tasks carry exactly one agent session: implementing one hands the work to
  // a new agent on a branch, which is a different work item entirely.
  if (offersAnotherAgent(kind, canAdd)) {
    bubbles.push({
      type: "add",
      id: "",
      label: "+",
      pattern: null,
      title: addAgentTitle(kind),
      active: selectedKind === "add",
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
 * Branches only: a task's one agent IS the task's conversation, so there is
 * nothing to remove there, only a task to abandon. Every agent on a branch
 * may go, the first and the last included — a branch left with none is a
 * working branch whose chat tab asks which agent to start one on.
 */
export function canRemoveAgent({ agents = [], agentId = null, kind = "branch" } = {}) {
  if ((kind !== "branch" && kind !== "workspace") || !agentId) return false;
  return agents.some((agent) => agent.id === agentId);
}

/** How removal names the agent it is about to take away — in the button's
 *  tooltip and in the confirmation behind it, which say the same words.
 *
 *  Quoted when the agent named its work, because the name is the agent's own
 *  and the prompt is only repeating it; pointed at ("this Claude Code agent")
 *  when it has not named one, because there is nothing yet to quote. */
export function agentRemovalWho(agent) {
  const heading = agentHeading(agent);
  return heading.starting ? `this ${providerLabel(agent && agent.provider)} agent` : `"${heading.text}"`;
}

/** The confirmation plan for `agent.remove` — the outline core/confirm.js asks
 *  with. Removal kills the agent's session and takes its conversation with it,
 *  so it says both, and says what it does NOT touch. */
export function removeAgentConfirm(agent, kind = "branch") {
  const who = agentRemovalWho(agent);
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
 *  exists, else the first they watch, else the first — the rail is never open
 *  on nothing. */
export function selectAgentId(agents = [], wanted = null) {
  if (wanted && agents.some((agent) => agent.id === wanted)) return wanted;
  return (agents.find(agentIsWatched) || agents[0])?.id ?? null;
}

/** The conversation a page was last left on, unless the reader does not watch
 *  it: its bubble went when they left (#105), so coming back does not bring it
 *  back. Kept while the agents are not known yet, to be asked again once they
 *  are. */
export const rememberedAgentId = (agents = [], remembered = null) =>
  agents.find((agent) => agent.id === remembered && !agentIsWatched(agent)) ? null : remembered;

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
  // A branch/task row may still carry the old entity aggregate. It is not an
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

/** A task: its own id, its one agent, its own conversation. */
const taskEntity = (row) => ({
  entityId: row.task_id || row.plan_id || null,
  kind: "task",
  projectId: row.project_id || null,
  branch: null,
  worktreeId: null,
  adoptable: false,
  canAdd: false,
  executionContext: row.execution_context || null,
  agents: row.agents || [],
  thread: row.thread || null,
});

/** A project: the conversation owner `project.ensure_conversation` minted, and
 *  nothing else. There is no checkout under a project to adopt — the project's
 *  own is the template its workspaces are cut from, and the agent works in a
 *  scratch directory Build owns — so the rail here is a conversation and no
 *  more: nothing to adopt, one agent, and no directory a written path could be
 *  resolved against. */
const projectEntity = (row, agents) => ({
  entityId: row.entity_id || row.run_id || null,
  kind: "project",
  projectId: row.project_id || null,
  branch: null,
  worktreeId: null,
  adoptable: false,
  canAdd: false,
  chatCapable: true,
  executionContext: row.execution_context || null,
  directories: [],
  agents,
  thread: row.thread || null,
});

/** A workspace: the owner it minted, and the checkouts mounted into it. */
const workspaceOwner = (row) => row.entity_id || row.run_id || (row.run && row.run.id) || null;
/** The one branch a workspace's row says it is on: its own, else the first of
 *  its sources that is on one. */
const workspaceBranch = (row) => row.branch || (row.directories || []).find((directory) => directory.branch)?.branch || null;
const workspaceThread = (row) => (row.run && row.run.thread) || row.thread || null;

const workspaceEntity = (row, agents) => {
  const entityId = workspaceOwner(row);
  return {
    entityId,
    kind: "workspace",
    projectId: row.project_id || null,
    branch: workspaceBranch(row),
    worktreeId: null,
    adoptable: false,
    canAdd: !!entityId && agents.length > 0,
    chatCapable: true,
    executionContext: row.execution_context || null,
    directories: row.directories || [],
    agents,
    thread: workspaceThread(row),
  };
};

/** A branch: its entity through the same helper the inbox uses
 *  (core/entityId.js), and its run's conversation when Build owns one. A branch
 *  with no run is a checkout nobody has claimed: the rail still shows a
 *  conversation, and sending in it is what adopts the checkout. */
const branchEntity = (payload, row, agents) => ({
  entityId: entityIdOf(payload ? { ...row, kind: "branch" } : null),
  kind: "branch",
  projectId: row.project_id || null,
  branch: row.branch || null,
  worktreeId: row.worktree_id || null,
  // No run behind the branch means no owner for an agent to report `done` to:
  // the first message adopts the checkout on its way to being sent.
  adoptable: !row.run_id,
  canAdd: !!row.run_id && agents.length > 0,
  chatCapable: true,
  executionContext: row.execution_context || null,
  agents,
  thread: (row.run && row.run.thread) || null,
});

/**
 * What the rail is the rail OF, read off one detail payload — one reader per
 * kind of work item, because each names its entity its own way.
 */
export function railEntity(payload, kind = "branch") {
  const row = payload || {};
  if (kind === "task") return taskEntity(row);
  const agents = row.agents || (row.run && row.run.agents) || [];
  if (kind === "project") return projectEntity(row, agents);
  if (kind === "workspace") return workspaceEntity(row, agents);
  return branchEntity(payload, row, agents);
}
