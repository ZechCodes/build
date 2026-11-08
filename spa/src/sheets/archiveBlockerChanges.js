import { cachedRecords, readCached, readCachedMany } from "../core/localCache.js";

const projectKinds = new Set(["project-settings", "tracker-tasks", "tracker-tasks-query", "tracker-tasks-page", "tracker-task", "tracker-summary", "task-review"]);
const rowId = row => row?.project_id || row?.id;
const keyOf = address => JSON.stringify([address.entityId, address.kind, address.sub || ""]);
const signatureOf = value => JSON.stringify(value, (_key, item) => item && !Array.isArray(item) && typeof item === "object"
  ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);

// Observe values, not write stamps: a refresh can announce an identical value.
// The sheet owns the subscription; this reader keeps each scoped sub-record
// separate and filters device lists to the project whose blockers were refused.
export function archiveBlockerChanges(deviceId, projectId, project, clear) {
  const signatures = new Map();
  const listAddress = kind => ({ deviceId, entityId: "", kind });
  const owner = () => project()?.entity_id || project()?.run_id;
  const listed = address => !address.entityId && ["projects", "workspaces"].includes(address.kind);
  const relevant = address => listed(address)
    || address.entityId === projectId && projectKinds.has(address.kind)
    || address.kind === "project-agents" && address.entityId === owner();
  function remember(address, value, notify) {
    const rows = listed(address) ? (value || []).filter(row => address.kind === "projects" ? rowId(row) === projectId : row.project_id === projectId) : value;
    const key = keyOf(address), signature = signatureOf(rows);
    if (notify && signatures.get(key) !== signature) clear();
    signatures.set(key, signature);
  }
  async function prime() {
    const records = await cachedRecords({ deviceId, entityId: projectId });
    const addresses = [...records.map(row => row.address).filter(relevant), listAddress("projects"), listAddress("workspaces")];
    const projects = (await readCached(listAddress("projects")))?.value || [];
    const owned = owner() || projects.find(row => rowId(row) === projectId)?.entity_id;
    if (owned) addresses.push({ deviceId, entityId: owned, kind: "project-agents" });
    const held = await readCachedMany(addresses);
    addresses.forEach((address, index) => remember(address, held[index]?.value, false));
  }
  let reads = prime();
  return address => {
    if (!relevant(address)) return;
    reads = reads.then(async () => remember(address, (await readCached(address))?.value, true));
  };
}
