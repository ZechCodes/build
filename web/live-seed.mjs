// Seed the live browser pass: two workspaces cut from the compose fixture
// repository, each with one agent conversation and one posted message. Prints
// one JSON line the browser pass (live-check.mjs) reads. Runs inside the qa
// image, which has the harness deps and the compose network.
//
//   docker compose -f deploy/compose.real.yml --profile qa run --rm qa node live-seed.mjs

import { randomUUID } from "node:crypto";
import * as transport from "@build/secure-transport";
import { openDeviceLink, openRendezvous } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const relayUrl = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const email = process.env.QA_EMAIL || "qa@localhost";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const { cookie, mintGatewayToken } = await loginWithDummy(apiUrl, { email });
const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken });
const link = await openDeviceLink({ rendezvous, transport, apiUrl, cookie });
const call = (method, params = {}) => link.session.call(method, params);

const projects = await call("project.list");
const project = projects.projects[0];
const models = await call("models.list");
const provider = models.default_provider || models.providers?.[0]?.id;
const model = models.models?.[0]?.id;

const tag = Date.now().toString(36);
const seeded = [];
for (const label of ["alpha", "beta"]) {
  const name = `live-${label}-${tag}`;
  let ws = await call("workspace.create", { project_id: project.project_id, name, isolation: "worktree" });
  for (let i = 0; i < 40 && ws.status !== "ready"; i++) {
    await sleep(250);
    ws = await call("workspace.get", { workspace_id: ws.workspace_id });
  }
  const conversation = await call("workspace.ensure_conversation", { workspace_id: ws.workspace_id, provider, model, effort: "medium" });
  const added = await call("agent.add", { entity_id: conversation.entity_id, creation_id: randomUUID(), provider, model, effort: "medium" });
  const agent = added.agent;
  const posted = await call("thread.post", {
    entity_id: conversation.entity_id,
    agent_id: agent.id,
    conversation_id: agent.conversation_id,
    operation_id: randomUUID(),
    body: `Live check: say hello from ${name}.`,
  });
  seeded.push({
    workspaceId: ws.workspace_id, name, status: ws.status, root: ws.root,
    gitDir: ws.directories?.[0]?.path || ws.root,
    entityId: conversation.entity_id, agentId: agent.id, conversationId: agent.conversation_id,
    postedSequence: posted.posted_sequence,
  });
}
// Let the QA agent answer before the browser looks.
await sleep(4000);
for (const s of seeded) {
  const page = await call("thread.page", { entity_id: s.entityId, agent_id: s.agentId, limit: 20 });
  s.threadItems = page.items?.length ?? 0;
}
console.log("SEED " + JSON.stringify({ deviceId: link.device.deviceId, projectId: project.project_id, provider, model, workspaces: seeded }));
link.close();
process.exit(0);
