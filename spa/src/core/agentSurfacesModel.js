import { workingClock } from "./agentRailModel.js";

export const SURFACE_KINDS = ["workflows", "subagents", "shells", "checklist"];

const KIND_LABELS = {
  workflows: "Workflows",
  subagents: "Subagents",
  shells: "Shells",
  checklist: "Checklist",
};

const AGENT_ENTRY_KIND = "subagents";

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
  workflows: RUN_STATE_MARKS,
  [AGENT_ENTRY_KIND]: AGENT_STATE_MARKS,
  shells: RUN_STATE_MARKS,
  checklist: {
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

function keyedRows(kind, entries, normalise) {
  const claimed = new Set();
  return entries.map((entry, index) => {
    const id = entry && entry.id ? String(entry.id) : "";
    const key = claimKey(kind, id, index, claimed);
    claimed.add(key);
    return { key, ...normalise(entry || {}, index) };
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
    id: entry.id || null,
    label: entry.label || "",
    model: entry.model || "",
    state: entry.state || "",
    stateMark: surfaceStateMark(AGENT_ENTRY_KIND, entry.state),
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
  return keyedRows("agent", Array.isArray(agents) ? agents : [], agentRow);
}

const ROW_NORMALISERS = {
  workflows: (entry) => ({
    id: entry.id || null,
    name: entry.name || "",
    description: entry.description || "",
    state: entry.state || "",
    stateMark: surfaceStateMark("workflows", entry.state),
    phaseCount: phasesOf(entry).length,
  }),
  shells: (entry) => ({
    id: entry.id || null,
    description: entry.description || "",
    state: entry.state || "",
    stateMark: surfaceStateMark("shells", entry.state),
    exitCode: Number.isFinite(entry.exit_code) ? entry.exit_code : null,
    tail: Array.isArray(entry.tail) ? entry.tail : [],
  }),
  checklist: (entry) => ({
    id: entry.id || null,
    subject: entry.subject || "",
    description: entry.description || "",
    state: entry.state || "",
    stateMark: surfaceStateMark("checklist", entry.state),
  }),
};

export function surfaceRows(kind, surfaces) {
  const entries = entriesOfKind(surfaces, kind);
  if (kind === AGENT_ENTRY_KIND) return agentRows(entries);
  const normalise = ROW_NORMALISERS[kind];
  return normalise ? keyedRows(kind, entries, normalise) : [];
}

export function workflowPhases(workflow, selectedIndex) {
  const phases = phasesOf(workflow);
  if (!phases.length) return { phases: [], agents: [] };
  const selected = selectedIndex >= 0 && selectedIndex < phases.length ? selectedIndex : 0;
  return {
    phases: keyedRows("phase", phases, (phase, index) => ({
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
  workflows: (entry) => entry.name || entry.description || entry.id || "",
  subagents: (entry) => entry.label || entry.id || "",
  shells: (entry) => entry.description || entry.id || "",
  checklist: (entry) => entry.subject || entry.description || entry.id || "",
};

const ROW_ACTIONS = {
  workflows: (subject) => [
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
  subagents: (subject) => [
    {
      id: "stop-subagent",
      label: "Ask to stop",
      description: "Ask the agent to stop this subagent",
      message: `Please stop the subagent "${subject}".`,
    },
    {
      id: "explain-subagent",
      label: "Ask what it is doing",
      description: "Ask the agent what this subagent is working on",
      message: `What is the subagent "${subject}" working on right now?`,
    },
  ],
  shells: (subject) => [
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
  checklist: (subject) => [
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
  return actionsOf(ROW_SUBJECTS[kind](entry));
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
    storage.setItem(OPEN_SURFACE_KEY_PREFIX + key, kind);
  } catch {
    return;
  }
}
