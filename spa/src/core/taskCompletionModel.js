const completed = (row) => row?.state === "completed";
const rememberedRow = (row) => ({ state: row.state, subject: String(row.subject || row.description || "") });

const checklistRows = (surfaces) => Array.isArray(surfaces?.checklist)
  ? surfaces.checklist.filter((row) => row && typeof row.id === "string" && row.id)
  : [];

const observationIsUsable = (observation) => !observation
  || (observation.support !== "unsupported" && observation.freshness === "current");

const trustworthy = (surfaces) => observationIsUsable(surfaces?.observations?.checklist)
  && surfaces?.checklist_provenance?.carried_from_prior_turn !== true;

const provenanceOf = (surfaces) => {
  const value = surfaces?.checklist_provenance;
  if (!value || typeof value !== "object") return null;
  const epoch = Number.isFinite(value.collection_epoch) ? value.collection_epoch : null;
  return {
    lineage: JSON.stringify([
      value.source ?? null,
      value.provider_session_generation ?? null,
      value.turn_id ?? null,
    ]),
    epoch,
  };
};

const completionToken = (lineage, row) =>
  JSON.stringify([lineage, row.id, rememberedRow(row).subject]);

const newAgentState = (generation) => ({
  generation, lineage: null, epoch: null, states: new Map(), announced: new Set(),
});

const establishBaseline = (held, lineage, epoch, rows) => {
  held.lineage = lineage;
  held.epoch = epoch;
  held.states = new Map(rows.map((row) => [row.id, rememberedRow(row)]));
  for (const row of rows.filter(completed)) held.announced.add(completionToken(lineage, row));
};

const isNewCompletion = (held, lineage, row) => {
  const previous = held.states.get(row.id);
  const sameTask = previous?.subject === rememberedRow(row).subject;
  return completed(row) && sameTask && !completed(previous)
    && !held.announced.has(completionToken(lineage, row));
};

const advance = (held, lineage, epoch, rows) => {
  if (epoch != null && held.epoch != null && epoch < held.epoch) return [];
  if (epoch != null) held.epoch = Math.max(held.epoch ?? epoch, epoch);
  const newlyCompleted = rows.filter((row) => isNewCompletion(held, lineage, row));
  held.states = new Map(rows.map((row) => [row.id, rememberedRow(row)]));
  for (const row of rows.filter(completed)) held.announced.add(completionToken(lineage, row));
  return newlyCompleted.map((row) => ({ id: row.id, title: String(row.subject || row.description || "Checklist item") }));
};

/**
 * Watches live agent snapshots for real pending -> completed transitions.
 * The first usable snapshot for an agent/lineage is only a baseline. Provider
 * provenance fences reused ids and the epoch watermark rejects replayed polls.
 */
export function createTaskCompletionTracker() {
  const agents = new Map();

  const stateFor = (agent) => {
    const generation = agent.surface_session_generation || null;
    const existing = agents.get(agent.id);
    if (existing?.generation === generation) return existing;
    const fresh = newAgentState(generation);
    agents.set(agent.id, fresh);
    return fresh;
  };

  const observeSnapshot = (held, surfaces) => {
    const rows = checklistRows(surfaces);
    const provenance = provenanceOf(surfaces);
    const lineage = provenance?.lineage || "legacy";
    if (held.lineage === lineage) return advance(held, lineage, provenance?.epoch ?? null, rows);
    establishBaseline(held, lineage, provenance?.epoch ?? null, rows);
    return [];
  };

  return {
    observe(agent) {
      if (!agent?.id) return [];
      const surfaces = agent.surfaces;
      if (!trustworthy(surfaces)) return [];
      return observeSnapshot(stateFor(agent), surfaces);
    },
    forgetMissing(agentIds) {
      const standing = new Set(agentIds);
      for (const id of agents.keys()) if (!standing.has(id)) agents.delete(id);
    },
    reset() {
      agents.clear();
    },
  };
}
