// Only cache records: this reproduction needs no bridge, account or pairing.
export const deviceId = "inbox-lineage-device";
export const projectId = "inbox-lineage-project";
export const entityId = "project-owner";
export const pinId = "project-coordinator";
export const topic = "Bridge roll and #564";

export function projectLineageFixture(now = Date.now()) {
  const projectKey = `${deviceId}/${projectId}`;
  const project = { id: projectId, name: "Build", entity_id: entityId, deviceId, projectKey };
  const pin = { id: pinId, conversation_id: "coordinator-history", ordinal: 1, provider: "codex",
    project_role: "coordinator", name: "Project agent", topic, state: "live", watched: true,
    working: false, unread_count: 0 };
  const row = (owner, agents) => ({ kind: "branch", entity_id: owner, run_id: owner,
    project_id: projectId, projectKey, deviceId, agents,
    session_started_ms: now - 100_000, last_activity_ms: now - 10_000 });
  const children = Array.from({ length: 3 }, (_, index) => row(`worker-owner-${index}`, [{
    id: `worker-${index}`, conversation_id: `worker-history-${index}`, name: `Worker ${index + 1}`,
    created_by: pinId, working: true, watched: false, unread_count: 0,
  }]));
  const workspaces = children.map((child, index) => ({ id: `workspace-${index}`, name: `Task workspace ${index + 1}`,
    entity_id: child.entity_id, project_id: projectId, deviceId, projectKey,
    workspaceKey: `${deviceId}/workspace-${index}`, status: "ready" }));
  return { project, pin, children, workspaces, owner: row(entityId, [pin]),
    roster: { entity_id: entityId, pinned_agent_id: pinId, agents: [pin] } };
}

export async function writeProjectLineageFixture(cache, fixture) {
  const write = (kind, value, owner = "") => cache.writeCached({ deviceId, entityId: owner, kind }, value);
  await write("projects", [fixture.project]);
  await write("workspaces", fixture.workspaces);
  await write("agent-lineage-support", { namesMakers: true });
  await write("project-agents-support", { projectWorkers: true });
  await write("project-agents", fixture.roster, entityId);
  await write("feed", { items: [fixture.owner], runs: [fixture.owner, ...fixture.children] });
  for (const row of [fixture.owner, ...fixture.children]) await write("row", row, row.entity_id);
  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId, name: "Fixture device" }]);
}
