// The account's selection and Recent expiry happen once. A roster landing
// changes only its owner's row; cache publication remains the reader's job.
import { cacheableEntityIds } from "./inbox.js";
import { projectRosterRows } from "./projectRosterFeed.js";

export function updateRosterSelection(view, pass, projectIndex, roster) {
  const entityId = roster.entity_id;
  const before = pass.rows.get(entityId);
  const base = pass.rosterBaseRows.get(entityId);
  const project = { ...view.projects[projectIndex], projectRoster: roster };
  const row = projectRosterRows({ projects: [project], items: base ? [base] : [] }).at(-1);
  const wasActive = pass.activeIds.has(entityId);
  const active = cacheableEntityIds({ items: [row] }).includes(entityId);
  const beforeCount = before?.agents?.length || 0;
  pass.selection.active += (active ? roster.agents.length : 0) - (wasActive ? beforeCount : 0);
  pass.selection.projectOwner += roster.agents.length - beforeCount;
  if (active) pass.activeIds.add(entityId);
  else pass.activeIds.delete(entityId);
  view.projects[projectIndex] = project;
  pass.rows.set(entityId, row);
  if (!before) pass.order.push(entityId);
}
