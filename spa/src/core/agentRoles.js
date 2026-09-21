// Which models this device is for which roles.
//
// The user declares a list: each row a model, the roles it can fill, and how
// much direction it needs. The ORDER is the preference — when two models can
// both review, the one nearer the top reviews — so the panel can move a row up
// and down, and that is a real edit rather than a cosmetic one.
//
// Capability is not an input to the choice. It is what the orchestrator is
// TOLD when it asks for a role, so it knows whether to write a goal, a scope
// or a list of steps.
//
// Pure: the shape and the words. The DOM is views' business, and the bridge
// holds the same rule in bridge/src/models.rs.

/** The roles a model can be declared for, in the order the panel lists them. */
export const AGENT_ROLES = Object.freeze([
  { id: "planner", label: "Planner", describes: "works out what to do and how to split it" },
  { id: "implementer", label: "Implementer", describes: "writes and changes the code" },
  { id: "reviewer", label: "Reviewer", describes: "reads a change and says what is wrong with it" },
  { id: "executor", label: "Executor", describes: "carries out a plan that already exists" },
]);

/** How much direction a model needs, said as what to do about it. */
export const AGENT_CAPABILITIES = Object.freeze([
  { id: "generalist", label: "Generalist", direction: "give it the goal; it needs little direction" },
  { id: "scoped", label: "Scoped", direction: "give it a clear scope and the constraints" },
  { id: "step_by_step", label: "Step by step", direction: "give it the steps; it infers little" },
]);

const rows = (models) => (Array.isArray(models) ? models : []);

/** What fills a role: the first row that can, because the list is in the
 *  user's preference order. `null` when nothing has been declared for it. */
export function modelForRole(models, role, capability = null) {
  return (
    rows(models).find(
      (row) =>
        (row.roles || []).includes(role) && (!capability || row.capability === capability),
    ) || null
  );
}

/** Every role that has a model, for a panel that wants to say which do not. */
export function rolesFilled(models) {
  const filled = new Set();
  for (const row of rows(models)) for (const role of row.roles || []) filled.add(role);
  return filled;
}

/** The list with one row's role toggled on or off. A new list every time: the
 *  panel holds what the bridge answered, and a refused save must leave it. */
export function withRole(models, index, role, on) {
  return rows(models).map((row, at) => {
    if (at !== index) return row;
    const roles = new Set(row.roles || []);
    if (on) roles.add(role);
    else roles.delete(role);
    return { ...row, roles: AGENT_ROLES.map((entry) => entry.id).filter((id) => roles.has(id)) };
  });
}

/** The list with one row's capability set. */
export function withCapability(models, index, capability) {
  return rows(models).map((row, at) => (at === index ? { ...row, capability } : row));
}

/** The list with a row moved one place, which is what changes the preference.
 *  A move off either end is no move at all. */
export function moved(models, index, by) {
  const list = [...rows(models)];
  const to = index + by;
  if (to < 0 || to >= list.length) return list;
  [list[index], list[to]] = [list[to], list[index]];
  return list;
}

/** The list with a row added, or removed. */
export function withModel(models, { provider = "", model, capability = "scoped" }) {
  return [...rows(models), { ...(provider ? { provider } : {}), model, roles: [], capability }];
}

export function withoutRow(models, index) {
  return rows(models).filter((_row, at) => at !== index);
}

/** Whether this list could be saved: every row needs an id, and a model may
 *  appear once — a second entry could never be reached, and the user would
 *  have no way to tell which one they were editing. */
export function whyUnsavable(models) {
  const seen = new Set();
  for (const row of rows(models)) {
    if (!String(row.model || "").trim()) return "Every model needs an id.";
    const key = `${row.provider || ""}|${row.model}`;
    if (seen.has(key)) return `${row.model} is in the list twice. A model appears once, with all of its roles.`;
    seen.add(key);
  }
  return "";
}

/** One line saying what this row is for, for the row's own hover. */
export function rowSummary(row) {
  const roles = (row.roles || [])
    .map((id) => AGENT_ROLES.find((role) => role.id === id)?.label || id)
    .join(", ");
  const capability = AGENT_CAPABILITIES.find((entry) => entry.id === row.capability);
  if (!roles) return "No roles — never chosen, but kept in the list.";
  return `${roles} · ${capability?.direction || row.capability}`;
}
