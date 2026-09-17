import { CHECKLIST_ENTRY_KIND, surfaceRows } from "./agentSurfacesModel.js";

const GOAL_STATUS = {
  active: "Goal active",
  paused: "Goal paused",
  blocked: "Goal blocked",
  usage_limited: "Goal usage limited",
  budget_limited: "Goal budget limited",
  complete: "Goal complete",
};

const observationOf = (surfaces, kind) => surfaces?.observations?.[kind] || null;
const isStale = (observation) => observation?.freshness === "stale";
const isCurrent = (observation) => observation?.freshness === "current";

const observationNotes = (observation, priorTurn = false) => [
  ...(isStale(observation) ? ["Last known"] : []),
  ...(priorTurn ? ["Prior turn"] : []),
  ...(observation?.coverage === "partial" ? ["Partial"] : []),
];

function goalStatus(state, live) {
  if (state === "active" && live) return "Goal active · Running";
  return GOAL_STATUS[state] || `Goal state: ${state || "unknown"}`;
}

function goalModel(surfaces, working) {
  const goal = surfaces?.goal;
  if (!goal || typeof goal !== "object") return null;
  const observation = observationOf(surfaces, "goal");
  const state = String(goal.state ?? "");
  const live = working === true && isCurrent(observation);
  return {
    objective: String(goal.objective ?? ""),
    status: goalStatus(state, live),
    notes: observationNotes(observation),
    stale: isStale(observation),
    live,
  };
}

function checklistProgress(rows, observation) {
  const completed = rows.filter((row) => row.state === "completed").length;
  if (observation?.coverage === "partial") {
    const omitted = Number.isFinite(observation.omitted_count) && observation.omitted_count > 0
      ? ` · ${observation.omitted_count} omitted`
      : "";
    return `${completed} known completed${omitted}`;
  }
  return `${completed}/${rows.length}`;
}

const currentChecklistRow = (rows) =>
  rows.find((row) => row.state === "in_progress") || rows.find((row) => row.state !== "completed");

function checklistModel(surfaces) {
  const observation = observationOf(surfaces, "checklist");
  if (!observation || observation.support === "unsupported") return null;
  const rows = surfaceRows(CHECKLIST_ENTRY_KIND, surfaces);
  if (!rows.length) return null;
  const priorTurn = surfaces?.checklist_provenance?.carried_from_prior_turn === true;
  return {
    rows,
    currentStep: currentChecklistRow(rows)?.subject || "",
    progress: checklistProgress(rows, observation),
    notes: observationNotes(observation, priorTurn),
    stale: isStale(observation),
    priorTurn,
    live: isCurrent(observation) && !isStale(observation) && !priorTurn,
  };
}

export function observationPanelModel(surfaces, { working = false } = {}) {
  return {
    goal: goalModel(surfaces, working),
    checklist: checklistModel(surfaces),
  };
}
