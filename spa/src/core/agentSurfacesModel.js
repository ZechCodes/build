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

const RUN_STATE_MARKS = {
  running: { mark: RUNNING_MARK, label: "Running" },
  done: { mark: DONE_MARK, label: "Done" },
  failed: { mark: "error", label: "Failed" },
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

export function surfacePills(surfaces) {
  return SURFACE_KINDS.map((kind) => ({ kind, entries: entriesOfKind(surfaces, kind) }))
    .filter(({ entries }) => entries.length > 0)
    .map(({ kind, entries }) => ({
      kind,
      label: KIND_LABELS[kind],
      count: entries.length,
      live: entries.some((entry) => stateMarkIs(kind, entry, RUNNING_MARK)),
    }));
}

export function openSurfaceKind(surfaces, wanted) {
  return surfacePills(surfaces).some((pill) => pill.kind === wanted) ? wanted : null;
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
