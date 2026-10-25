// Device-wide cached ownership and policy for watched tasks. Task lists have
// their own project readers; an agent hint never reloads those lists or policy.
import { subscribeCache } from "./localCache.js";
import { contextFor } from "./deviceContexts.js";
import { createFeedCacheReader } from "./feedCacheReader.js";
import { needsYouRuleAddress, readNeedsYouRule } from "./needsYouRule.js";
import { agentLineage, lineageMembers } from "./agentLineageModel.js";
import { projectCoordinatorAgent, projectRosterRows } from "./projectRosterFeed.js";
import { entityIdOf } from "./entityId.js";
import { trailingRead } from "./trailingRead.js";

const AGENT_KINDS = new Set(["row", "projects", "workspaces", "agent-lineage-support", "project-agents", "project-agents-support"]);
const sessionOf = (deviceId) => contextFor(deviceId)?.session || null;

function runningFor(project, feed, rows, namesMakers) {
  const members = lineageMembers(rows, { projectId: project.id, projects: feed.projects, workspaces: feed.workspaces });
  const lineage = agentLineage(members, { namesMakers });
  const owner = feed.projects.find((candidate) => (candidate.id || candidate.project_id) === project.id) || project;
  const ownerId = owner.entity_id || owner.run_id;
  const agents = rows.find((row) => row.project_id === project.id && entityIdOf(row) === ownerId)?.agents || [];
  return {
    runningAgentIds: new Set(members.filter(({ agent }) => lineage.rollup(agent.id).running).map(({ agent }) => agent.id)),
    projectAgentId: ownerId ? projectCoordinatorAgent(owner, agents)?.id || null : null,
  };
}

export function createWatchedTaskDependencies(deviceId) {
  const listeners = new Set();
  const reader = createFeedCacheReader(deviceId);
  let disposed = false;
  let invalidation = 0;
  let ruleInvalidation = 0;
  let ruleVersion = 0;
  let askedOnly = false;
  let workspaceSignature = "[]";
  let workspaceVersion = 0;
  let workspaces = [];

  function publish(listener) {
    if (!listener.ruleReady || !listener.agentsReady) return;
    const { runningAgentIds, projectAgentId } = listener.running;
    const signature = JSON.stringify([askedOnly, [...runningAgentIds].sort(), projectAgentId, workspaceVersion]);
    if (listener.signature === signature) return;
    listener.signature = signature;
    listener.onChange({ askedOnly, runningAgentIds, projectAgentId, workspaces });
  }

  function acceptWorkspaces(next) {
    if (workspaces === next) return;
    const signature = JSON.stringify(next);
    workspaces = next;
    if (signature === workspaceSignature) return;
    workspaceSignature = signature;
    workspaceVersion += 1;
  }

  async function readAgents() {
    if (disposed) return;
    const session = sessionOf(deviceId);
    const before = invalidation;
    const { view, namesMakers } = await reader.refresh();
    if (disposed || before !== invalidation || session !== sessionOf(deviceId)) return;
    acceptWorkspaces(view.workspaces);
    const rows = projectRosterRows(view);
    for (const listener of listeners) {
      listener.running = runningFor(listener.project, view, rows, namesMakers);
      listener.agentsReady = true;
      publish(listener);
    }
  }

  async function readRule() {
    if (disposed) return;
    const session = sessionOf(deviceId);
    const before = ruleInvalidation;
    const version = ruleVersion;
    const next = await readNeedsYouRule(deviceId);
    if (disposed || before !== ruleInvalidation || session !== sessionOf(deviceId)) return;
    askedOnly = next;
    for (const listener of listeners) {
      if (version >= listener.ruleVersion) listener.ruleReady = true;
      publish(listener);
    }
  }

  const options = { generationOf: () => sessionOf(deviceId) };
  const refreshAgents = trailingRead(readAgents, options);
  const refreshRule = trailingRead(readRule, options);
  const unsubscribeAgents = subscribeCache({ deviceId }, (address) => {
    if (!address.kind) invalidation += 1;
    if (!address.kind || AGENT_KINDS.has(address.kind)) void refreshAgents();
  });
  const unsubscribeRule = subscribeCache(needsYouRuleAddress(deviceId), (address) => {
    ruleVersion += 1;
    if (!address.kind) ruleInvalidation += 1;
    void refreshRule();
  });

  return {
    subscribe(project, onChange) {
      const listener = { project, onChange, signature: null, agentsReady: false, ruleReady: false, ruleVersion, running: { runningAgentIds: new Set(), projectAgentId: null } };
      listeners.add(listener);
      void refreshAgents();
      void refreshRule();
      return () => listeners.delete(listener);
    },
    dispose() {
      disposed = true;
      listeners.clear();
      unsubscribeAgents();
      unsubscribeRule();
    },
  };
}
