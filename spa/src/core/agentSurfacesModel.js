import { elapsedClock, runningClock } from "./agentRailModel.js";

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

const PENDING_STATE = "pending";
const RUNNING_STATE = "running";
const DONE_STATE = "done";

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

function graceExpiryFor(visibility, kind, nowMs) {
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
  return graceExpiryFor(visibility, kind, nowMs) !== null;
}

/** A snapshot read back off disk: the kinds that linger describe a live tab
 *  the harness may no longer have, so once their grace has run out they are
 *  dropped rather than painted and taken away again a moment later. */
export function surfacesAfterGrace(surfaces, seenAtMs, nowMs) {
  if (!surfaces || seenAtMs + SURFACE_PILL_GRACE_MS >= nowMs) return surfaces;
  const kept = { ...surfaces };
  for (const kind of KINDS_THAT_LINGER) delete kept[kind];
  return kept;
}

export function surfaceKindLabel(kind) {
  return KIND_LABELS[kind] || "";
}

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
    .map((kind) => graceExpiryFor(visibility, kind, nowMs))
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

function keyedBy(keyPrefix, entries, shape) {
  const claimed = new Set();
  return entries.map((given, index) => {
    const entry = given || {};
    const key = claimKey(keyPrefix, entry.id ? String(entry.id) : "", index, claimed);
    claimed.add(key);
    return { key, ...shape(entry, index) };
  });
}

export function entryClock(startedAt, durationMs, running, nowMs) {
  if (running && Number.isFinite(startedAt)) {
    return { clock: elapsedClock(startedAt, nowMs), runningSince: startedAt };
  }
  if (Number.isFinite(durationMs)) return { clock: runningClock(durationMs / 1000), runningSince: null };
  return { clock: "", runningSince: null };
}

function keyedRows(keyPrefix, entryKind, entries, normalise, { nowMs = 0 } = {}) {
  return keyedBy(keyPrefix, entries, (entry, index) => ({
    id: entry.id || null,
    state: entry.state || "",
    stateMark: surfaceStateMark(entryKind, entry.state),
    subject: rowSubject(entryKind, entry),
    ...entryClock(entry.started_at, entry.duration_ms, stateMarkIs(entryKind, entry, RUNNING_MARK), nowMs),
    ...normalise(entry, index),
  }));
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

function agentRow(entry, modelLabel) {
  return {
    label: entry.label || "",
    model: entry.model ? modelLabel(entry.model) : "",
    tokens: Number.isFinite(entry.tokens) ? entry.tokens : null,
    toolCalls: Number.isFinite(entry.tool_calls) ? entry.tool_calls : null,
    lastTool: lastToolText(entry.last_tool),
    result: entry.result || "",
    error: entry.error || "",
    attempt: Number.isFinite(entry.attempt) ? entry.attempt : null,
    callSequence: Number.isFinite(entry.call_sequence) ? entry.call_sequence : null,
  };
}

const rawModelId = (modelId) => modelId;

export function agentRows(agents, reading = {}) {
  const { modelLabel = rawModelId } = reading;
  const entries = Array.isArray(agents) ? agents : [];
  return keyedRows("agent", AGENT_ENTRY_KIND, entries, (entry) => agentRow(entry, modelLabel), reading);
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

export function surfaceRows(kind, surfaces, reading = {}) {
  const entries = entriesOfKind(surfaces, kind);
  if (kind === AGENT_ENTRY_KIND) return agentRows(entries, reading);
  const normalise = ROW_NORMALISERS[kind];
  return normalise ? keyedRows(kind, kind, entries, normalise, reading) : [];
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

export function openWorkflow(surfaces, selectedWorkflowIndex = 0, reading = {}) {
  const rows = surfaceRows(WORKFLOW_ENTRY_KIND, surfaces, reading);
  if (!rows.length) return null;
  return rows[chosenIndex(rows.length, selectedWorkflowIndex)];
}

export function workflowChoicesWorthOffering(surfaces, selectedWorkflowIndex, reading = {}) {
  const rows = surfaceRows(WORKFLOW_ENTRY_KIND, surfaces, reading);
  if (rows.length < 2) return [];
  const selected = chosenIndex(rows.length, selectedWorkflowIndex);
  return rows.map((row, index) => ({ ...row, index, selected: index === selected }));
}

export function phaseClock(agents, running, nowMs) {
  const { startedAt, endedAt } = phaseSpan(agents);
  return entryClock(startedAt, startedAt === null ? null : endedAt - startedAt, running, nowMs);
}

function phaseSpan(agents) {
  return agents.reduce(
    (span, agent) => {
      if (!Number.isFinite(agent.started_at)) return span;
      const endedAt = agent.started_at + (Number.isFinite(agent.duration_ms) ? agent.duration_ms : 0);
      if (span.startedAt === null) return { startedAt: agent.started_at, endedAt };
      return { startedAt: Math.min(span.startedAt, agent.started_at), endedAt: Math.max(span.endedAt, endedAt) };
    },
    { startedAt: null, endedAt: null },
  );
}

const rowMarkIs = (row, mark) => !!row.stateMark && row.stateMark.mark === mark;

function phaseState(rows, running) {
  if (running) return RUNNING_STATE;
  return rows.length && rows.every(rowHasFinished) ? DONE_STATE : PENDING_STATE;
}

export function workflowPhases(surfaces, selectedWorkflowIndex = 0, reading = {}) {
  const phases = phasesOf(selectedWorkflowEntry(surfaces, selectedWorkflowIndex));
  return keyedBy("phase", phases, (phase) => {
    const agents = agentsOf(phase);
    const rows = agentRows(agents, reading);
    const running = rows.some((row) => rowMarkIs(row, RUNNING_MARK));
    return {
      title: phase.title || "",
      total: rows.length,
      done: rows.filter((row) => rowMarkIs(row, DONE_MARK)).length,
      state: phaseState(rows, running),
      open: running,
      ...phaseClock(agents, running, reading.nowMs || 0),
      rows,
    };
  });
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
