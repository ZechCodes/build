import { mountLayout } from "./layoutHarness.mjs";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { terminalShim } from "./projectWorkspaceTerminalFixture.mjs";

// Replace the terminal transport/WASM only. The console, source authorization,
// cache, rail, editor, Git pane and assignment picker are production modules.
export const flowTerminalShim = {
  ...terminalShim,
  load(id) {
    if (id === "\0project-terminal-manager") return `
      export const terminalManager=()=>window.__flowTerminalManager;
      export const terminalDeviceId=()=>window.__layoutModules.app.App.route.deviceId;
      export const subscribeTerminalStatus=()=>()=>{};
      export const provideTerminalSessions=()=>{};
      export const followTerminalDevice=()=>{};
      export const resetTerminalManager=()=>{};
      export const createStatusHub=()=>({subscribe:()=>()=>{}});`;
    return terminalShim.load(id);
  },
};

export async function mountProjectFlow(page, basePath, options = {}) {
  page.setDefaultTimeout(7000);
  await mountLayout(page, `<div id="shell"><div id="view"><header id="toolbar">Build</header>
    <div id="view-body"><nav id="dir-rail" aria-label="Project views"></nav><main id="root"></main>
    <aside id="agent-rail" aria-label="Agents"></aside></div><div id="console-region"></div></div></div>`,
  { basePath, styles: "#shell{--inbox-space:0px;height:100vh} #toolbar{padding:12px 20px} .project-terminal-screen{padding:16px;white-space:pre-wrap;font-size:13px}" });
  await loadChatOverviewModules(page, basePath, {
    project: "src/views/projectView.js", shell: "src/core/shell.js", contexts: "src/core/deviceContexts.js",
    events: "src/core/changeEvents.js", directory: "src/core/directoryScope.js", scope: "src/core/cacheScope.js",
    changesSupport: "src/core/projectChangesSupport.js", terminalSupport: "src/core/projectTerminalSupport.js",
    filesSupport: "src/core/projectFilesSupport.js", roster: "src/core/projectAgentsCache.js",
    terminalRpc: "src/core/projectTerminalRpc.js", terminalCache: "src/core/projectTerminalCache.js",
    assignee: "src/core/trackerAssignee.js", picker: "src/core/trackerAssigneePicker.js",
    returnRoute: "src/core/projectRailState.js", router: "src/core/router.js",
  });
  await page.evaluate(seedFlow, { supported: true, greeting: true, theme: "dark", ...options });
  await page.locator('[data-agent="project-pin"]').waitFor();
  await page.locator('[data-directory="code"]').waitFor();
}

async function seedFlow({ supported, greeting, theme }) {
  const { app, cache, feed, contexts, events, directory, scope, project, shell, merge,
    changesSupport, terminalSupport, filesSupport, roster, terminalRpc } = window.__layoutModules;
  document.documentElement.dataset.theme = theme;
  document.head.insertAdjacentHTML("afterbegin", '<meta name="viewport" content="width=device-width, initial-scale=1">');
  const patch = "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-old wording\n+shared source wording\n";
  const agent = (id, name, purpose) => ({ id, name, topic: name, project_role: purpose,
    conversation_id: `conversation-${id}`, provider: "codex", ordinal: purpose === "coordinator" ? 1 : 2,
    state: "live", watched: true, working: false, unread_count: 0 });
  const pin = agent("project-pin", "Build coordinator", "coordinator");
  const sibling = agent("project-sibling", "Docs worker", "worker");
  const sources = [
    { id: "code", name: "Code", path: "/fixture/code", is_git: true, base_branch: "main", sync_base: true,
      sync: { status: "skipped", reason: "local commits" } },
    { id: "docs", name: "Docs", path: "/fixture/docs", is_git: true, base_branch: "main" },
    { id: "assets", name: "Assets", path: "/fixture/assets", is_git: false },
    { id: "unborn", name: "New repo", path: "/fixture/unborn", is_git: true, base_branch: "main" },
  ];
  const dirtyFile = { path: "README.md", staged: "none", index_status: "M", worktree_status: "M",
    content_key: "identical-content-key", added: 1, deleted: 1 };
  const status = (sourceId, dirty = true) => ({ branch: "main", status_key: "identical-status-key",
    files: dirty ? [{ ...dirtyFile, index_status: sourceId === "unborn" ? "?" : "M" }] : [],
    stat: { files_changed: dirty ? 1 : 0, insertions: dirty ? 1 : 0, deletions: dirty ? 1 : 0 },
    repo_state: "clean", ahead: sourceId === "code" ? 1 : 0, behind: 0, stash_count: 0 });
  const recentCommit = { hash: "d".repeat(40), short: "ddddddd", subject: "Local base commit", author: "Developer", time: 1791900000 };
  window.__flowCalls = [];
  window.__flowDevices = {};
  for (const deviceId of ["flow-device", "other-device"]) {
    const label = deviceId === "flow-device" ? "Build" : "Other computer";
    const record = { id: "p", project_id: "p", entity_id: "project-owner", deviceId,
      projectKey: `${deviceId}/p`, name: label, path: sources[0].path, sources, agents: [pin] };
    window.__flowDevices[deviceId] = { supported, record,
      roster: { entity_id: "project-owner", pinned_agent_id: pin.id, agents: [sibling, pin] },
      files: new Map(sources.map((source) => [source.id, `# ${label} ${source.id} source\n`])),
      statuses: new Map(sources.filter((source) => source.is_git).map((source) => [source.id, status(source.id)])),
      terminals: new Map(sources.map((source) => [source.id, []])),
    };
  }
  const supportFor = (enabled) => enabled ? ["fs.projectSources", "fs.projectSourcePath", "git.projectSources",
    "term.projectSources", "changes.projectSources", "agents.projectWorkers", "tasks.assignProjectAgent",
    "sources.syncBase", "sources.syncBasePath"] : [];
  window.__flowCall = async (deviceId, method, params = {}) => {
    const held = window.__flowDevices[deviceId];
    window.__flowCalls.push({ deviceId, method, params });
    if (!held.supported && ["source_id", "source_path", "include_project_workers", "project_role"].some((key) => Object.hasOwn(params, key))) {
      throw new Error("Legacy protocol fixture refuses project source and worker fields");
    }
    const sourceId = params.source_id || "code";
    if (method === "session.hello") return { api_version: held.supported ? "3.22.0" : "3.20.0", capabilities: supportFor(held.supported), push_events: false };
    if (method === "models.list") return { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] };
    if (method === "agent.list") {
      if (params.entity_id === "child-owner") return { entity_id: "child-owner", agents: [agent("child-agent", "Isolated editor", null)] };
      if (params.include_project_workers) return held.roster;
      if (held.supported) return { ...held.roster, agents: [pin] };
      const legacyPin = { ...pin };
      delete legacyPin.project_role;
      return { entity_id: "project-owner", agents: [legacyPin] };
    }
    if (method === "agent.add") {
      const created = agent("project-created", params.name, "worker");
      held.roster = { ...held.roster, agents: [created, ...held.roster.agents] };
      return { entity_id: "project-owner", agent: created };
    }
    if (method === "agent.remove") {
      held.roster = { ...held.roster, agents: held.roster.agents.filter((one) => one.id !== params.agent_id) };
      return { removed: true, entity_id: "project-owner", agent_id: params.agent_id };
    }
    if (method === "thread.post") return { entity_id: params.entity_id, agent_id: params.agent_id,
      conversation_id: params.conversation_id, posted_sequence: 2 };
    if (method === "thread.get" || method === "thread.list") return { items: [] };
    if (method === "agent.start") return { agent_id: params.agent_id };
    if (method === "tasks.assign") {
      const task = { id: params.task_id, number: 538, title: "Accept project workspace", project_id: "p",
        assignee: params.assignee, identities: { "project-created": { name: "Release helper", available: true } },
        links: { workspace_ids: [], branches: [], commits: [], conversation_ids: ["conversation-project-created"] } };
      window.__flowAssigned = task;
      return { task, dispatch: { agent_id: params.assignee.agent_id, conversation_id: "conversation-project-created" } };
    }
    if (method === "git.status") return held.statuses.get(sourceId);
    if (method === "git.log") return { commits: sourceId === "unborn" ? [] : [recentCommit] };
    if (method === "git.diff") return { patch, files: [{ ...dirtyFile, patch, truncated: false }] };
    if (method === "git.refs") return { current: { name: "main", kind: "branch" }, refs: [{ name: "main", full_ref: "refs/heads/main", kind: "branch", current: true }] };
    if (method === "git.stage") return {};
    if (method === "git.commit") {
      held.statuses.set(sourceId, status(sourceId, false));
      return { status: held.statuses.get(sourceId) };
    }
    if (method === "fs.tree") return { path: params.path, entries: [{ name: "README.md", kind: "file", size: 50 }] };
    if (method === "fs.read") return { path: params.path, mime: "text/markdown", content_b64: btoa(held.files.get(sourceId)),
      size: held.files.get(sourceId).length, editable: true, encoding: "utf-8", revision: "revision-1" };
    if (method === "fs.write") {
      held.files.set(sourceId, atob(params.content_b64));
      return { path: params.path, revision: "revision-2" };
    }
    if (method === "term.list") return { tabs: held.terminals.get(sourceId) || [] };
    if (method === "term.create") {
      const created = { term_id: `term-${sourceId}`, name: "Source shell" };
      held.terminals.get(sourceId).push(created);
      return created;
    }
    if (method === "term.attach") return { snapshot: `Shell on ${deviceId} in ${params.source_path}\n$ `, cursor: 0 };
    if (method === "term.close") { held.terminals.set(sourceId, []); return {}; }
    return {};
  };
  window.__flowGreet = async (deviceId = "flow-device", enabled = true) => {
    window.__flowDevices[deviceId].supported = enabled;
    const call = (method, params) => window.__flowCall(deviceId, method, params);
    const context = contexts.adoptDeviceSession({ deviceId, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await events.greetBridge(call, { deviceId, install: (selection) => {
      const adapter = selection.create(call);
      contexts.adoptBridgeSelection(context, selection, adapter);
      return adapter;
    } });
  };
  const terminalCall = (method, params) => {
    const deviceId = app.App.route.deviceId;
    const context = contexts.knownDeviceContext(deviceId);
    if (params.source_id) return terminalRpc.projectTerminalRpc(context, method, params);
    return context.rpc(method, params);
  };
  window.__flowTerminalManager = {
    listTerminals: async (at) => (await terminalCall("term.list", at)).tabs,
    createTerminal: (at) => terminalCall("term.create", at),
    closeTerminal: (termId, at) => terminalCall("term.close", { ...at, term_id: termId }),
    attachTerminal: (termId, at) => terminalCall("term.attach", { ...at, term_id: termId }),
    detach() {}, input() {}, resize() {},
  };
  app.App.devices = [{ id: "flow-device", name: "Fixture computer" }, { id: "other-device", name: "Other fixture" }];
  app.App.selectedDeviceId = "flow-device";
  app.App.route = { name: "project", deviceId: "flow-device", projectId: "p", tab: "changes", sourceId: "code" };
  await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
  for (const [deviceId, held] of Object.entries(window.__flowDevices)) {
    await changesSupport.rememberProjectChangesSupport(deviceId, { git: { projectSources: supported }, projects: { syncBase: supported, syncBasePath: supported } });
    await terminalSupport.rememberProjectTerminalSupport(deviceId, { term: { projectSources: supported } });
    await filesSupport.rememberProjectFilesSupport(deviceId, { fs: { projectSources: supported } });
    await roster.rememberProjectAgentsSupport(deviceId, { agents: { projectWorkers: supported }, tasks: { assignProjectAgent: supported } });
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [held.record]);
    await cache.writeCached({ deviceId, entityId: "project-owner", kind: "project-agents" }, held.roster);
    await cache.writeCached({ deviceId, entityId: "project-owner", kind: "row" }, { kind: "project", entity_id: "project-owner", project_id: "p", agents: [pin] });
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [merge.stampWorkspace({ id: "child", workspace_id: "child", project_id: "p", entity_id: "child-owner", name: "Local commit workspace", status: "ready" }, deviceId)]);
    await cache.writeCached({ deviceId, entityId: "child-owner", kind: "row" }, { kind: "workspace", entity_id: "child-owner", project_id: "p", workspace_id: "child", agents: [agent("child-agent", "Isolated editor", null)] });
    for (const person of held.roster.agents) await cache.writeCached({ deviceId, entityId: "project-owner", kind: "thread", sub: person.conversation_id },
      { items: [{ type: "message", data: { sequence: 1, role: "agent", body: `Cached ${held.record.name}: ${person.name}`, created_at: "2026-10-12T18:00:00Z" } }] });
    for (const source of sources) {
      const entityId = directory.directoryCacheId({ project_id: "p", source_id: source.id, source_path: source.path });
      await cache.writeCached({ deviceId, entityId, kind: "terminals" }, { tabs: [] });
      if (source.is_git) {
        await cache.writeCached({ deviceId, entityId, kind: "status" }, held.statuses.get(source.id));
        await cache.writeCached({ deviceId, entityId, kind: "log" }, { commits: source.id === "unborn" ? [] : [recentCommit] });
      }
    }
    if (!supported) {
      const entityId = directory.directoryCacheId({ project_id: "p" });
      await cache.writeCached({ deviceId, entityId, kind: "status" }, held.statuses.get("code"));
      await cache.writeCached({ deviceId, entityId, kind: "log" }, { commits: [recentCommit] });
    }
    if (greeting) await window.__flowGreet(deviceId, supported);
  }
  await feed.startFeed();
  shell.standShell(app.App.route);
  await project.renderProject();
  window.__flowNavigate = async (route) => {
    app.App.viewDispose?.(); app.App.viewDispose = null;
    app.App.route = route;
    history.replaceState(null, "", window.__layoutModules.router.hashFromRoute(route));
    shell.standShell(route);
    if (route.name === "project") await project.renderProject();
    else document.querySelector("#root").textContent = "Local commit workspace · Changes";
  };
  // Used for production RPC and cache operations in the compatibility matrix.
  window.__flowDependencies = { scope, contexts };
}
