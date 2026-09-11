// Full-stack QA over the E2EE relay. Exercises multi-source workspaces,
// source-scoped files/Git, workspace-scoped terminals, and retained finish.
//
// Compose provides BRIDGE_REPO=/repo and BRIDGE_WORKTREES=/worktrees. This
// script creates isolated remote and plain-folder fixtures through a terminal.
// Usage: API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node qa.mjs

import WebSocket from "ws";
import * as transport from "@build/secure-transport";
import { openSession, openPushSession } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const encode = (text) => Buffer.from(text, "utf8").toString("base64");
const decode = (text) => Buffer.from(text || "", "base64").toString("utf8");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const relayUrl = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;

let passed = 0;
const checks = [];
function check(name, condition, detail = "") {
  checks.push({ name, ok: !!condition });
  if (condition) passed++;
  console.log(`${condition ? "✓" : "✗"} ${name}${detail ? `  — ${detail}` : ""}`);
}

async function waitFor(predicate, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = predicate();
    if (result) return result;
    await sleep(50);
  }
  return null;
}

function connect() {
  const ws = new WebSocket(`${relayUrl}/ws/client`);
  const queue = [];
  const waiters = [];
  ws.on("message", (data) => {
    const message = JSON.parse(data.toString());
    waiters.length ? waiters.shift()(message) : queue.push(message);
  });
  const recv = () => new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("recv timeout")), 10000);
    const deliver = (message) => {
      clearTimeout(timer);
      resolve(message);
    };
    queue.length ? deliver(queue.shift()) : waiters.push(deliver);
  });
  const ready = new Promise((resolve, reject) => {
    ws.on("open", resolve);
    ws.on("error", reject);
  });
  return { ws, recv, ready, send: (message) => ws.send(JSON.stringify(message)) };
}

async function authenticate(mintGatewayToken) {
  const connection = connect();
  await connection.ready;
  connection.send({ type: "authenticate", token: await mintGatewayToken() });
  return { connection, ack: await connection.recv() };
}

function pushedText(pushes, termId) {
  return pushes
    .filter((push) => ["term.output", "term.reset"].includes(push.type) && push.term_id === termId)
    .map((push) => decode(push.data))
    .join("");
}

async function main() {
  const { mintGatewayToken } = await loginWithDummy(apiUrl, {
    email: process.env.QA_EMAIL || "qa@localhost",
  });
  const { connection: rpcConnection, ack } = await authenticate(mintGatewayToken);
  check("relay accepts the gateway token", ack.type === "authenticated", `got ${ack.type}`);
  const { call } = await openSession({
    send: rpcConnection.send,
    recv: rpcConnection.recv,
    transport,
    preferDeviceId,
  });
  check("ping round-trips over E2EE", (await call("ping")).pong === true);

  const projectList = await call("project.list");
  const fixtureProject = projectList.projects?.[0];
  check("the compose fixture project is registered", !!fixtureProject, fixtureProject?.project_id || "none");
  if (!fixtureProject) throw new Error("BRIDGE_REPO fixture project is unavailable");

  const primaryList = await call("workspace.list", { project_id: fixtureProject.project_id });
  const primary = primaryList.workspaces.find((workspace) =>
    workspace.directories.some((directory) => directory.path === "/repo"));
  check("workspace.list adopts BRIDGE_REPO", !!primary, primary?.workspace_id || "none");
  if (!primary) throw new Error("the /repo workspace is unavailable");

  const { connection: termConnection, ack: termAck } = await authenticate(mintGatewayToken);
  check("terminal socket accepts the gateway token", termAck.type === "authenticated");
  const pushes = [];
  const term = await openPushSession({
    send: termConnection.send,
    recv: termConnection.recv,
    transport,
    preferDeviceId,
    onPush: (push) => pushes.push(push),
  });

  const setupTerm = await term.call("term.create", { workspace_id: primary.workspace_id, cols: 100, rows: 30 });
  await term.call("term.attach", { workspace_id: primary.workspace_id, term_id: setupTerm.term_id, cols: 100, rows: 30 });
  const tag = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const remotePath = `/worktrees/qa-origin-${tag}.git`;
  const plainPath = `/tmp/qa-assets-${tag}`;
  const setupMarker = `qa-setup-${tag}`;
  const setup = [
    `git clone --bare /repo ${remotePath}`,
    `mkdir -p ${plainPath}`,
    `printf 'original\\n' > ${plainPath}/logo.txt`,
    `printf '%s%s\\n' 'qa-setup-' '${tag}'`,
  ].join(" && ");
  await term.call("term.input", { term_id: setupTerm.term_id, data: encode(`${setup}\r`) });
  check("workspace terminal prepares isolated QA sources", !!(await waitFor(() => pushedText(pushes, setupTerm.term_id).includes(setupMarker))));
  await term.call("term.close", { term_id: setupTerm.term_id });

  const project = await call("project.add", {
    name: `qa-mixed-${tag}`,
    sources: [
      { name: "api", remote: `file://${remotePath}` },
      { name: "assets", path: plainPath },
    ],
  });
  check("project.add accepts remote and path sources", !!project.project_id, project.project_id);

  const workspace = await call("workspace.create", {
    project_id: project.project_id,
    name: `qa-workspace-${tag}`,
    isolation: "worktree",
  });
  check("workspace.create returns a ready workspace", workspace.status === "ready", `status=${workspace.status}`);
  check("workspace.create materializes both sources", workspace.directories?.length === 2);
  const gitDirectory = workspace.directories.find((directory) => directory.source_id === "source-1");
  const plainDirectory = workspace.directories.find((directory) => directory.source_id === "source-2");
  check("directories preserve source identity and capability", gitDirectory?.is_git === true && plainDirectory?.is_git === false);

  const listed = await call("workspace.list", { project_id: project.project_id });
  check("workspace.list returns the created workspace", listed.workspaces.some((item) => item.workspace_id === workspace.workspace_id));
  const got = await call("workspace.get", { workspace_id: workspace.workspace_id });
  check("workspace.get preserves both identity fields", got.id === got.workspace_id && got.id === workspace.workspace_id);
  let readyRetryRejected = false;
  try {
    await call("workspace.retry", { workspace_id: workspace.workspace_id });
  } catch (error) {
    readyRetryRejected = /no failed provisioning/.test(error.message);
  }
  check("workspace.retry refuses a workspace that is already ready", readyRetryRejected);

  const plainScope = { workspace_id: workspace.workspace_id, source_id: plainDirectory.source_id };
  const tree = await call("fs.tree", plainScope);
  check("fs.tree reads the paired plain source", tree.entries.some((entry) => entry.name === "logo.txt"));
  const opened = await call("fs.read", { ...plainScope, path: "logo.txt" });
  check("fs.read returns the fixture bytes", decode(opened.content_b64) === "original\n");
  const replacement = "workspace copy\n";
  const written = await call("fs.write", {
    ...plainScope,
    path: "logo.txt",
    expected_revision: opened.revision,
    content_b64: encode(replacement),
  });
  check("fs.write updates the paired source copy", decode(written.content_b64) === replacement);
  let traversalRejected = false;
  try {
    await call("fs.read", { ...plainScope, path: "../../../etc/passwd" });
  } catch (error) {
    traversalRejected = /escapes/.test(error.message);
  }
  check("fs.read fences traversal paths", traversalRejected);

  const gitScope = { workspace_id: workspace.workspace_id, source_id: gitDirectory.source_id };
  const refs = await call("git.refs", gitScope);
  check("Git calls accept the workspace/source pair", Array.isArray(refs.refs));
  const gitReadme = await call("fs.read", { ...gitScope, path: "README.md" });
  const gitReplacement = `${decode(gitReadme.content_b64).trimEnd()}\n\nQA workspace ${tag}\n`;
  await call("fs.write", {
    ...gitScope,
    path: "README.md",
    expected_revision: gitReadme.revision,
    content_b64: encode(gitReplacement),
  });
  const dirty = await call("git.status", gitScope);
  check("Git status sees a source-scoped workspace edit", dirty.files?.some((file) => file.path === "README.md"));
  await call("git.stage", { ...gitScope, paths: ["README.md"] });
  const committed = await call("git.commit", { ...gitScope, message: `QA workspace ${tag}` });
  check("Git commit records the workspace edit", committed.subject === `QA workspace ${tag}`);
  let plainGitRejected = false;
  try {
    await call("git.refs", plainScope);
  } catch (error) {
    plainGitRejected = /not a git repository/.test(error.message);
  }
  check("Git calls reject the paired plain source", plainGitRejected);
  let missingSourceRejected = false;
  try {
    await call("git.status", { workspace_id: workspace.workspace_id });
  } catch (error) {
    missingSourceRejected = /source_id/.test(error.message);
  }
  check("Git workspace scope requires source_id", missingSourceRejected);

  const workspaceTerm = await term.call("term.create", { workspace_id: workspace.workspace_id, cols: 80, rows: 24 });
  const attached = await term.call("term.attach", {
    workspace_id: workspace.workspace_id,
    term_id: workspaceTerm.term_id,
    cols: 80,
    rows: 24,
  });
  check("workspace terminal attaches with snapshot and cursor", typeof attached.snapshot === "string" && typeof attached.cursor === "number");
  const terminalMarker = `qa-term-${tag}`;
  await term.call("term.input", {
    term_id: workspaceTerm.term_id,
    data: encode(`printf '%s%s\\n' 'qa-term-' '${tag}'\r`),
  });
  check("workspace terminal echoes over the relay", !!(await waitFor(() => pushedText(pushes, workspaceTerm.term_id).includes(terminalMarker))));
  const terminals = await term.call("term.list", { workspace_id: workspace.workspace_id });
  check("term.list uses workspace_id only", terminals.terminals.some((item) => item.term_id === workspaceTerm.term_id));

  const finished = await call("workspace.finish", { workspace_id: workspace.workspace_id });
  const gitFinish = finished.repositories.find((item) => item.directory_id === gitDirectory.id);
  check("workspace.finish pushes each Git directory", finished.complete === true && gitFinish?.pushed === true);
  check("finish results pair by directory_id", finished.repositories.every((item) => workspace.directories.some((directory) => directory.id === item.directory_id)));
  const afterFinish = await call("workspace.get", { workspace_id: workspace.workspace_id });
  check("finish retains the workspace and marks it finished", afterFinish.status === "finished" && afterFinish.root === workspace.root);
  const retained = await call("fs.read", { ...plainScope, path: "logo.txt" });
  check("finished workspace files remain available", decode(retained.content_b64) === replacement);
  const retainedTerminals = await term.call("term.list", { workspace_id: workspace.workspace_id });
  check(
    "finish retains live workspace terminals",
    retainedTerminals.terminals.some((item) => item.term_id === workspaceTerm.term_id),
  );
  await term.call("term.close", { term_id: workspaceTerm.term_id });

  const localProject = await call("project.create", { name: `qa-local-${tag}` });
  const localWorkspace = await call("workspace.create", {
    project_id: localProject.project_id,
    name: `qa-local-workspace-${tag}`,
    isolation: "worktree",
  });
  const incomplete = await call("workspace.finish", { workspace_id: localWorkspace.workspace_id });
  check("finish without a remote returns complete:false", incomplete.complete === false && incomplete.repositories.some((item) => item.pushed === false));
  const retainedIncomplete = await call("workspace.get", { workspace_id: localWorkspace.workspace_id });
  check("incomplete finish retains a ready workspace", retainedIncomplete.status === "ready" && retainedIncomplete.root === localWorkspace.root);

  let unknownRejected = false;
  try { await call("does.not.exist"); } catch (error) { unknownRejected = /unknown method/.test(error.message); }
  check("unknown method returns a clean error", unknownRejected);
  let missingWorkspaceRejected = false;
  try { await call("workspace.get", {}); } catch (error) { missingWorkspaceRejected = /workspace_id/.test(error.message); }
  check("missing workspace_id returns a clean error", missingWorkspaceRejected);

  termConnection.ws.close();
  rpcConnection.ws.close();
  const failed = checks.filter((result) => !result.ok);
  console.log(`\n${passed}/${checks.length} checks passed`);
  if (failed.length) {
    console.error("QA FAIL:", failed.map((result) => result.name).join("; "));
    process.exit(1);
  }
  console.log("QA PASS: workspaces verified end-to-end over E2EE");
  process.exit(0);
}

main().catch((error) => {
  console.error("QA ERROR:", error.message);
  process.exit(1);
});
