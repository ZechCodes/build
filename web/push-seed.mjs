// The container half of the browser push check (#191, web/push-check.mjs).
// Runs inside the qa image, on the compose network, as the paired user.
//
//   node push-seed.mjs seed            → one JSON line: a watched workspace
//                                        agent and a watched task to push about
//   node push-seed.mjs agent '<seed>'  → ask the agent something; the scripted
//                                        QA agent answers, which is the news
//   node push-seed.mjs own '<seed>'    → the user comments on the task, which
//                                        is their own doing and must stay quiet
//   node push-seed.mjs thread '<seed>' → what the agent's conversation holds

import { randomUUID } from "node:crypto";
import * as transport from "@build/secure-transport";
import { openDeviceLink, openRendezvous } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const relayUrl = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const email = process.env.QA_EMAIL || "qa@localhost";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const { cookie, mintGatewayToken } = await loginWithDummy(apiUrl, { email });
const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl, cookie });
const call = (method, params = {}) => link.session.call(method, params);

async function seed() {
  const project = (await call("project.list")).projects[0];
  const models = await call("models.list");
  const provider = models.default_provider || models.providers?.[0]?.id;
  const model = models.models?.[0]?.id;
  const name = `push-${Date.now().toString(36)}`;
  let workspace = await call("workspace.create", { project_id: project.project_id, name, isolation: "worktree" });
  for (let i = 0; i < 40 && workspace.status !== "ready"; i++) {
    await sleep(250);
    workspace = await call("workspace.get", { workspace_id: workspace.workspace_id });
  }
  const conversation = await call("workspace.ensure_conversation", { workspace_id: workspace.workspace_id, provider, model, effort: "medium" });
  const { agent } = await call("agent.add", { entity_id: conversation.entity_id, creation_id: randomUUID(), provider, model, effort: "medium" });
  const { task } = await call("tasks.create", { project_id: project.project_id, title: "Push check" });
  return {
    projectId: project.project_id,
    workspaceId: workspace.workspace_id,
    entityId: conversation.entity_id,
    agentId: agent.id,
    conversationId: agent.conversation_id,
    taskId: task.id,
    taskWatched: task.watched === true,
  };
}

async function askTheAgent(seeded) {
  await call("thread.post", {
    entity_id: seeded.entityId,
    agent_id: seeded.agentId,
    conversation_id: seeded.conversationId,
    operation_id: randomUUID(),
    body: "Push check: say hello.",
  });
  return { asked: seeded.agentId };
}

async function commentAsTheUser(seeded) {
  await call("tasks.comment", { task_id: seeded.taskId, body: "My own words, which push nothing." });
  return { commented: seeded.taskId };
}

async function thread(seeded) {
  const page = await call("thread.page", { entity_id: seeded.entityId, agent_id: seeded.agentId, limit: 20 });
  return (page.items || []).map((item) => JSON.stringify(item).slice(0, 300));
}

const [command, seedJson] = process.argv.slice(2);
const actions = { seed, agent: askTheAgent, own: commentAsTheUser, thread };
if (!actions[command]) throw new Error(`usage: node push-seed.mjs seed|agent|own|thread ['<seed json>']`);
const result = await actions[command](seedJson ? JSON.parse(seedJson) : null);
console.log(`PUSH ${JSON.stringify(result)}`);
link.close?.();
rendezvous.close?.();
process.exit(0);
