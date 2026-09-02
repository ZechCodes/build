import { workingClock } from "./agentRailModel.js";

export const WORKFLOW_ENTRY_KIND = "workflows";
export const AGENT_ENTRY_KIND = "subagents";
export const SHELL_ENTRY_KIND = "shells";
export const CHECKLIST_ENTRY_KIND = "checklist";

export const SURFACE_KINDS = [
  WORKFLOW_ENTRY_KIND,
  AGENT_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
];

const KIND_LABELS = {
  [WORKFLOW_ENTRY_KIND]: "Workflows",
  [AGENT_ENTRY_KIND]: "Subagents",
  [SHELL_ENTRY_KIND]: "Shells",
  [CHECKLIST_ENTRY_KIND]: "Checklist",
};

const RUNNING_MARK = "running";
const DONE_MARK = "ok";
const FAILED_MARK = "error";
const FINISHED_MARKS = [DONE_MARK, FAILED_MARK];

const RUN_STATE_MARKS = {
  running: { mark: RUNNING_MARK, label: "Running" },
  done: { mark: DONE_MARK, label: "Done" },
  failed: { mark: FAILED_MARK, label: "Failed" },
};

const AGENT_STATE_MARKS = {
  queued: { mark: "pending", label: "Queued" },
  ...RUN_STATE_MARKS,
};

const STATE_MARKS_BY_KIND = {
  [WORKFLOW_ENTRY_KIND]: RUN_STATE_MARKS,
  [AGENT_ENTRY_KIND]: AGENT_STATE_MARKS,
  [SHELL_ENTRY_KIND]: RUN_STATE_MARKS,
  [CHECKLIST_ENTRY_KIND]: {
    pending: { mark: "pending", label: "Pending" },
    in_progress: { mark: RUNNING_MARK, label: "In progress" },
    completed: { mark: DONE_MARK, label: "Completed" },
    blocked: { mark: "blocked", label: "Blocked" },
  },
};

const OPEN_SURFACE_KEY_PREFIX = "build.agentSurfaces.open.";

export const SURFACE_PILL_GRACE_MS = 60000;

const KINDS_THAT_LINGER = [AGENT_ENTRY_KIND, SHELL_ENTRY_KIND];

export function surfaceStateMark(kind, state) {
  const marks = STATE_MARKS_BY_KIND[kind];
  if (!marks) return null;
  return marks[state] || null;
}

function entriesOfKind(surfaces, kind) {
  if (!surfaces || !SURFACE_KINDS.includes(kind)) return [];
  const entries = surfaces[kind];
  return Array.isArray(entries) ? entries : [];
}

function stateMarkIs(kind, entry, mark) {
  const stateMark = surfaceStateMark(kind, entry && entry.state);
  return !!stateMark && stateMark.mark === mark;
}

export function emptySurfaceVisibility() {
  return { openKind: null, kinds: {} };
}

function kindVisibility(visibility, kind) {
  const kinds = (visibility && visibility.kinds) || {};
  return kinds[kind] || {};
}

export function advanceSurfaceVisibility(visibility, surfaces, nowMs) {
  const kinds = { ...((visibility && visibility.kinds) || {}) };
  for (const kind of KINDS_THAT_LINGER) {
    if (!runningEntryCount(surfaces, kind)) continue;
    kinds[kind] = { ...kindVisibility(visibility, kind), lastRunningSeenAt: nowMs };
  }
  return { openKind: (visibility && visibility.openKind) || null, kinds };
}

export function openedSurfaceVisibility(visibility, kind, nowMs) {
  const wasOpen = (visibility && visibility.openKind) || null;
  const opening = kind || null;
  if (wasOpen === opening) return visibility || emptySurfaceVisibility();
  const kinds = { ...((visibility && visibility.kinds) || {}) };
  if (wasOpen) kinds[wasOpen] = { ...kindVisibility(visibility, wasOpen), closedAt: nowMs };
  return { openKind: opening, kinds };
}

function runningEntryCount(surfaces, kind) {
  return entriesOfKind(surfaces, kind).filter((entry) => stateMarkIs(kind, entry, RUNNING_MARK)).length;
}

/** When the last grace this kind is holding runs out, or null once none is. */
function graceEnd(visibility, kind, nowMs) {
  const { lastRunningSeenAt, closedAt } = kindVisibility(visibility, kind);
  const ends = [lastRunningSeenAt, closedAt]
    .filter((at) => Number.isFinite(at))
    .map((at) => at + SURFACE_PILL_GRACE_MS)
    .filter((end) => end > nowMs);
  return ends.length ? Math.max(...ends) : null;
}

function pillIsShown(kind, runningCount, visibility, nowMs) {
  if (!KINDS_THAT_LINGER.includes(kind)) return true;
  if (runningCount > 0) return true;
  if (visibility && visibility.openKind === kind) return true;
  return graceEnd(visibility, kind, nowMs) !== null;
}

export function surfaceKindLabel(kind) {
  return KIND_LABELS[kind] || "";
}

/** The kinds this snapshot has something in, each with what is running in it.
 *  The one count in the client: the pills filter this by their grace rule, the
 *  conversation menu lists all of it. */
function kindsWithContent(surfaces) {
  return SURFACE_KINDS.filter((kind) => entriesOfKind(surfaces, kind).length > 0).map((kind) => ({
    kind,
    label: surfaceKindLabel(kind),
    count: runningEntryCount(surfaces, kind),
  }));
}

export function surfacePills(surfaces, visibility = null, nowMs = 0) {
  return kindsWithContent(surfaces).filter(({ kind, count }) => pillIsShown(kind, count, visibility, nowMs));
}

/** The conversation menu's options, shaped for `menuButtonMarkup`: every kind
 *  with content, whatever the pills' grace would say about it, since a reader
 *  asking for a surface by name is asking for the one they remember. */
export function surfaceMenuOptions(surfaces) {
  return kindsWithContent(surfaces).map(({ kind, label, count }) => ({
    id: kind,
    label,
    description: count ? `${count} running` : "",
  }));
}

export function nextSurfacePillExpiry(surfaces, visibility, nowMs) {
  const expiries = KINDS_THAT_LINGER.filter(
    (kind) =>
      entriesOfKind(surfaces, kind).length > 0 &&
      !runningEntryCount(surfaces, kind) &&
      !(visibility && visibility.openKind === kind),
  )
    .map((kind) => graceEnd(visibility, kind, nowMs))
    .filter((end) => end !== null);
  return expiries.length ? Math.min(...expiries) : null;
}

export function openSurfaceKind(surfaces, wanted, visibility = null, nowMs = 0) {
  return surfacePills(surfaces, visibility, nowMs).some((pill) => pill.kind === wanted) ? wanted : null;
}

function claimKey(kind, id, index, claimed) {
  if (id && !claimed.has(id)) return id;
  let fallbackIndex = index;
  while (claimed.has(`${kind}-${fallbackIndex}`)) fallbackIndex += 1;
  return `${kind}-${fallbackIndex}`;
}

function keyedRows(keyPrefix, markKind, entries, normalise) {
  const claimed = new Set();
  return entries.map((given, index) => {
    const entry = given || {};
    const key = claimKey(keyPrefix, entry.id ? String(entry.id) : "", index, claimed);
    claimed.add(key);
    if (!markKind) return { key, ...normalise(entry, index) };
    return {
      key,
      id: entry.id || null,
      state: entry.state || "",
      stateMark: surfaceStateMark(markKind, entry.state),
      subject: rowSubject(markKind, entry),
      actions: rowActions(markKind, entry),
      ...normalise(entry, index),
    };
  });
}

function phasesOf(workflow) {
  return workflow && Array.isArray(workflow.phases) ? workflow.phases : [];
}

function agentsOf(phase) {
  return phase && Array.isArray(phase.agents) ? phase.agents : [];
}

function lastToolText(lastTool) {
  if (!lastTool) return "";
  return [lastTool.name, lastTool.summary].filter(Boolean).join(" ");
}

function agentRow(entry) {
  return {
    label: entry.label || "",
    model: entry.model || "",
    duration: Number.isFinite(entry.duration_ms) ? workingClock(entry.duration_ms / 1000) : "",
    tokens: Number.isFinite(entry.tokens) ? entry.tokens : null,
    toolCalls: Number.isFinite(entry.tool_calls) ? entry.tool_calls : null,
    lastTool: lastToolText(entry.last_tool),
    result: entry.result || "",
    error: entry.error || "",
    attempt: Number.isFinite(entry.attempt) ? entry.attempt : null,
    callSequence: Number.isFinite(entry.call_sequence) ? entry.call_sequence : null,
  };
}

export function agentRows(agents) {
  return keyedRows("agent", AGENT_ENTRY_KIND, Array.isArray(agents) ? agents : [], agentRow);
}

const ROW_NORMALISERS = {
  [WORKFLOW_ENTRY_KIND]: (entry) => ({
    name: entry.name || "",
    description: entry.description || "",
    phaseCount: phasesOf(entry).length,
  }),
  [SHELL_ENTRY_KIND]: (entry) => ({
    description: entry.description || "",
    exitCode: Number.isFinite(entry.exit_code) ? entry.exit_code : null,
    tail: Array.isArray(entry.tail) ? entry.tail : [],
  }),
  [CHECKLIST_ENTRY_KIND]: (entry) => ({
    description: entry.description || "",
  }),
};

export function surfaceRows(kind, surfaces) {
  const entries = entriesOfKind(surfaces, kind);
  if (kind === AGENT_ENTRY_KIND) return agentRows(entries);
  const normalise = ROW_NORMALISERS[kind];
  return normalise ? keyedRows(kind, kind, entries, normalise) : [];
}

function rowHasFinished(row) {
  return !!row.stateMark && FINISHED_MARKS.includes(row.stateMark.mark);
}

export function runningAndCompletedRows(rows) {
  return {
    running: rows.filter((row) => !rowHasFinished(row)),
    completed: rows.filter(rowHasFinished),
  };
}

function chosenIndex(count, wantedIndex) {
  return wantedIndex >= 0 && wantedIndex < count ? wantedIndex : 0;
}

function selectedWorkflowEntry(surfaces, selectedWorkflowIndex) {
  const entries = entriesOfKind(surfaces, WORKFLOW_ENTRY_KIND);
  if (!entries.length) return null;
  return entries[chosenIndex(entries.length, selectedWorkflowIndex)];
}

export function openWorkflow(surfaces, selectedWorkflowIndex = 0) {
  const rows = surfaceRows(WORKFLOW_ENTRY_KIND, surfaces);
  if (!rows.length) return null;
  return rows[chosenIndex(rows.length, selectedWorkflowIndex)];
}

export function workflowChoicesWorthOffering(surfaces, selectedWorkflowIndex) {
  const rows = surfaceRows(WORKFLOW_ENTRY_KIND, surfaces);
  if (rows.length < 2) return [];
  const selected = chosenIndex(rows.length, selectedWorkflowIndex);
  return rows.map((row, index) => ({ ...row, index, selected: index === selected }));
}

export function workflowPhases(surfaces, selectedWorkflowIndex = 0, selectedPhaseIndex = 0) {
  const phases = phasesOf(selectedWorkflowEntry(surfaces, selectedWorkflowIndex));
  if (!phases.length) return { phases: [], agents: [] };
  const selected = chosenIndex(phases.length, selectedPhaseIndex);
  return {
    phases: keyedRows("phase", null, phases, (phase, index) => ({
      index,
      title: phase.title || "",
      total: agentsOf(phase).length,
      done: agentsOf(phase).filter((agent) => stateMarkIs(AGENT_ENTRY_KIND, agent, DONE_MARK)).length,
      selected: index === selected,
    })),
    agents: agentRows(agentsOf(phases[selected])),
  };
}

const ROW_SUBJECTS = {
  [WORKFLOW_ENTRY_KIND]: (entry) => entry.name || entry.description || entry.id || "",
  [AGENT_ENTRY_KIND]: (entry) => entry.label || entry.id || "",
  [SHELL_ENTRY_KIND]: (entry) => entry.description || entry.id || "",
  [CHECKLIST_ENTRY_KIND]: (entry) => entry.subject || entry.description || entry.id || "",
};

export function rowSubject(kind, entry) {
  const subjectOf = ROW_SUBJECTS[kind];
  if (!subjectOf || !entry) return "";
  return subjectOf(entry);
}

const ROW_ACTIONS = {
  [WORKFLOW_ENTRY_KIND]: (subject) => [
    {
      id: "stop-workflow",
      label: "Ask to stop",
      description: "Ask the agent to stop this workflow",
      message: `Please stop the workflow "${subject}".`,
    },
    {
      id: "explain-workflow",
      label: "Ask what it is doing",
      description: "Ask the agent what this workflow is working on",
      message: `What is the workflow "${subject}" working on right now?`,
    },
  ],
  [AGENT_ENTRY_KIND]: (subject) => [
    {
      id: "stop-agent",
      label: "Ask to stop",
      description: "Ask the agent to stop this one",
      message: `Please stop the agent "${subject}".`,
    },
    {
      id: "explain-agent",
      label: "Ask what it is doing",
      description: "Ask what this agent is working on",
      message: `What is the agent "${subject}" working on right now?`,
    },
  ],
  [SHELL_ENTRY_KIND]: (subject) => [
    {
      id: "stop-shell",
      label: "Ask to stop",
      description: "Ask the agent to stop this background command",
      message: `Please stop the background command "${subject}".`,
    },
    {
      id: "report-shell",
      label: "Ask for its output",
      description: "Ask the agent what this background command has printed",
      message: `What has the background command "${subject}" printed so far?`,
    },
  ],
  [CHECKLIST_ENTRY_KIND]: (subject) => [
    {
      id: "start-item",
      label: "Ask to work on it",
      description: "Ask the agent to take this item next",
      message: `Please work on "${subject}" next.`,
    },
    {
      id: "explain-item",
      label: "Ask about it",
      description: "Ask the agent where this item stands",
      message: `Where does "${subject}" stand?`,
    },
  ],
};

export function rowActions(kind, entry) {
  const actionsOf = ROW_ACTIONS[kind];
  if (!actionsOf || !entry) return [];
  return actionsOf(rowSubject(kind, entry));
}

export function readOpenSurface(key, storage = globalThis.localStorage) {
  try {
    const stored = storage.getItem(OPEN_SURFACE_KEY_PREFIX + key);
    return SURFACE_KINDS.includes(stored) ? stored : null;
  } catch {
    return null;
  }
}

export function writeOpenSurface(key, kind, storage = globalThis.localStorage) {
  try {
    if (kind) storage.setItem(OPEN_SURFACE_KEY_PREFIX + key, kind);
    else storage.removeItem(OPEN_SURFACE_KEY_PREFIX + key);
  } catch {
    return;
  }
}
