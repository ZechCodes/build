// Full-stack QA over the peer connection. Exercises multi-source workspaces,
// source-scoped files/Git, workspace-scoped terminals, and retained finish.
//
// Every check below rides this device's DataChannels: the relay socket is a
// rendezvous that mints the two sessions, carries the negotiation, and is closed
// before the first check runs (strict P2P transport spec, rules 2 and 4). Two
// checks are about the transport itself rather than the workspace — that the
// socket really is shut, and that the bridge refuses app RPC offered to it.
//
// Compose provides BRIDGE_REPO=/repo and BRIDGE_WORKTREES=/worktrees. This
// script creates isolated remote and plain-folder fixtures through a terminal.
// Usage: API_URL=http://127.0.0.1:8090 RELAY_URL=ws://127.0.0.1:18090 node qa.mjs

import * as transport from "@build/secure-transport";
import { openDeviceLink, openRelaySignalingSession, openRendezvous } from "./client.mjs";
import { loginWithDummy } from "./skrift-auth.mjs";

const encode = (text) => Buffer.from(text, "utf8").toString("base64");
const decode = (text) => Buffer.from(text || "", "base64").toString("utf8");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const relayUrl = process.env.RELAY_URL || "ws://127.0.0.1:18090";
const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const preferDeviceId = process.env.PREFER_DEVICE_ID || null;

let passed = 0;
import { FINISH_BLOCKERS, everyDirectoryIsARepository, plainDirectoriesOf, refusedBecause } from "./finishGate.mjs";

const checks = [];
function check(name, condition, detail = "") {
  checks.push({ name, ok: !!condition });
  if (condition) passed++;
  console.log(`${condition ? "✓" : "✗"} ${name}${detail ? `  — ${detail}` : ""}`);
}

/// Poll until the predicate answers something truthy, or give up.
///
/// The result is awaited: a predicate that asks the bridge something answers a
/// promise, and a promise is truthy whatever it later resolves to — so without
/// this an async predicate "succeeds" on its first tick and hands back the
/// promise instead of the answer.
async function waitFor(predicate, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await predicate();
    if (result) return result;
    await sleep(50);
  }
  return null;
}

/** Rule 1, from the client's side: the relay carries `rtc.*` and nothing else,
 *  and a bridge that is offered app RPC over it answers the refusal rather than
 *  dispatching. This is the only session in the suite that is not a peer's. */
async function relayRefusesAppRpc({ mintGatewayToken, device }) {
  const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken });
  try {
    const signaling = await openRelaySignalingSession({ rendezvous, transport, device });
    await signaling.call("session.hello", { client: { name: "qa", version: "0", api_range: ">=1.0.0 <2.0.0" } });
    return null;
  } catch (error) {
    return error;
  } finally {
    rendezvous.close();
  }
}

function pushedText(pushes, termId) {
  return pushes
    .filter((push) => ["term.output", "term.reset"].includes(push.type) && push.term_id === termId)
    .map((push) => decode(push.data))
    .join("");
}

async function main() {
  const { cookie, mintGatewayToken } = await loginWithDummy(apiUrl, {
    email: process.env.QA_EMAIL || "qa@localhost",
  });
  const rendezvous = await openRendezvous({ relayUrl, mintGatewayToken });
  check("relay accepts the gateway token", rendezvous.authenticated, `got ${rendezvous.ack.type}`);

  // One rendezvous, two sessions, one peer connection: the app session rides
  // the `app` channel and the terminals' rides `term`, exactly as the SPA does.
  const pushes = [];
  const link = await openDeviceLink({
    rendezvous,
    transport,
    apiUrl,
    cookie,
    preferDeviceId,
    terminal: { onPush: (push) => pushes.push(push) },
  });
  const { call } = link.session;
  const term = link.terminalSession;
  check("ping round-trips over E2EE", (await call("ping")).pong === true);
  check(
    "the relay socket is closed once the channels carry",
    rendezvous.isClosed(),
    `readyState=${rendezvous.socket.readyState}`,
  );
  check(
    "the terminals ride a second session on the same rendezvous",
    !!term && term.sessionId !== link.session.sessionId,
    `app=${link.session.sessionId} term=${term?.sessionId}`,
  );
  const refused = await relayRefusesAppRpc({ mintGatewayToken, device: link.device });
  check(
    "app RPC offered to the relay is refused, not carried",
    refused?.error_code === "unavailable" &&
      refused?.retryable === false &&
      refused?.details?.reason === "relay_is_not_a_data_plane",
    refused ? `${refused.message} (${refused.error_code}, ${JSON.stringify(refused.details)})` : "the relay carried it",
  );

  const projectList = await call("project.list");
  const fixtureProject = projectList.projects?.[0];
  check("the compose fixture project is registered", !!fixtureProject, fixtureProject?.project_id || "none");
  if (!fixtureProject) throw new Error("BRIDGE_REPO fixture project is unavailable");

  // The project's own checkout is a template, never listed as a workspace
  // (workspace.list omits it), so the setup shell runs in a workspace cut from
  // it: a worktree of /repo, which sees the same history to clone from.
  const primaryList = await call("workspace.list", { project_id: fixtureProject.project_id });
  check("workspace.list keeps the project's own checkout out", !primaryList.workspaces.some((workspace) =>
    workspace.directories.some((directory) => directory.path === "/repo")));
  const primary = await call("workspace.create", {
    project_id: fixtureProject.project_id,
    name: `qa-setup-${Date.now().toString(36)}`,
    isolation: "worktree",
  });
  check("a workspace is cut from BRIDGE_REPO for the setup shell", !!primary?.workspace_id, primary?.workspace_id || "none");
  if (!primary?.workspace_id) throw new Error("no workspace to run the setup shell in");

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
  // Done removes the workspace, so the bridge refuses one whose work is only
  // in it. The fixture has a bare clone of its own to publish to, so the
  // harness satisfies that honestly rather than finishing work nothing else
  // holds: the commit goes to the remote first.
  const published = await call("git.push", gitScope);
  check(
    "git.push publishes the workspace commit and sets the upstream",
    published.ahead === 0 && typeof published.upstream === "string" && published.upstream.length > 0,
    `upstream=${published.upstream} ahead=${published.ahead}`,
  );
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
  check("workspace terminal echoes over the DataChannel", !!(await waitFor(() => pushedText(pushes, workspaceTerm.term_id).includes(terminalMarker))));
  const terminals = await term.call("term.list", { workspace_id: workspace.workspace_id });
  check("term.list uses workspace_id only", terminals.terminals.some((item) => item.term_id === workspaceTerm.term_id));

  // The commit is published now, so the only thing left between this workspace
  // and Done is the plain folder it holds. Ask, and hold the gate to saying so:
  // a plain directory is a blocker in itself, because nothing measures it and
  // no remote has a copy.
  check("the workspace still holds a directory that is not a repository", plainDirectoriesOf(workspace).length === 1);
  let plainBlocked = null;
  try {
    await call("workspace.finish", { workspace_id: workspace.workspace_id });
  } catch (error) {
    plainBlocked = error;
  }
  check(
    "workspace.finish refuses a workspace holding a plain directory",
    refusedBecause(plainBlocked, FINISH_BLOCKERS.plainDirectory),
    plainBlocked?.message || "finish was not refused",
  );

  // Read the plain source while it is still here: it leaves the workspace next,
  // and what a FINISHED workspace keeps is asked of the Git directory below.
  const beforeRemoval = await call("fs.read", { ...plainScope, path: "logo.txt" });
  check("the workspace copy of the plain source holds the edit", decode(beforeRemoval.content_b64) === replacement);
  await call("workspace.remove_directory", {
    workspace_id: workspace.workspace_id,
    directory_id: plainDirectory.id,
  });
  const readyToFinish = await waitFor(async () => {
    const seen = await call("workspace.get", { workspace_id: workspace.workspace_id });
    return everyDirectoryIsARepository(seen) ? seen : null;
  });
  check("workspace.remove_directory leaves only repositories behind", !!readyToFinish, "the plain directory did not leave");

  const finished = await call("workspace.finish", { workspace_id: workspace.workspace_id });
  const gitFinish = finished.repositories.find((item) => item.directory_id === gitDirectory.id);
  check("workspace.finish pushes each Git directory", finished.complete === true && gitFinish?.pushed === true);
  check("finish results pair by directory_id", finished.repositories.every((item) => readyToFinish.directories.some((directory) => directory.id === item.directory_id)));
  const afterFinish = await call("workspace.get", { workspace_id: workspace.workspace_id });
  check("finish retains the workspace and marks it finished", afterFinish.status === "finished" && afterFinish.root === workspace.root);
  const retained = await call("fs.read", { ...gitScope, path: "README.md" });
  check("finished workspace files remain available", decode(retained.content_b64) === gitReplacement);
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
  // A project with no remote: its workspace is initialized with a commit and
  // nowhere to publish it, which is exactly what the gate is for. It used to
  // answer `complete:false`; since the gate it refuses, and refusing is the
  // better answer — Done would have removed the only copy.
  let localBlocked = null;
  try {
    await call("workspace.finish", { workspace_id: localWorkspace.workspace_id });
  } catch (error) {
    localBlocked = error;
  }
  check(
    "workspace.finish refuses a workspace no remote has a copy of",
    refusedBecause(localBlocked, FINISH_BLOCKERS.unpushed),
    localBlocked?.message || "finish was not refused",
  );
  const retainedIncomplete = await call("workspace.get", { workspace_id: localWorkspace.workspace_id });
  check("a refused finish leaves the workspace ready and untouched", retainedIncomplete.status === "ready" && retainedIncomplete.root === localWorkspace.root);

  let unknownRejected = false;
  try { await call("does.not.exist"); } catch (error) { unknownRejected = /unknown method/.test(error.message); }
  check("unknown method returns a clean error", unknownRejected);
  let missingWorkspaceRejected = false;
  try { await call("workspace.get", {}); } catch (error) { missingWorkspaceRejected = /workspace_id/.test(error.message); }
  check("missing workspace_id returns a clean error", missingWorkspaceRejected);

  link.close();
  const failed = checks.filter((result) => !result.ok);
  console.log(`\n${passed}/${checks.length} checks passed`);
  if (failed.length) {
    console.error("QA FAIL:", failed.map((result) => result.name).join("; "));
    process.exit(1);
  }
  console.log("QA PASS: workspaces verified end-to-end over the DataChannels");
  process.exit(0);
}

main().catch((error) => {
  console.error("QA ERROR:", error.message);
  process.exit(1);
});
