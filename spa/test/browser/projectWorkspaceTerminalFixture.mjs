import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

const SHELL = `<div id="shell"><div id="view"><div id="toolbar">Build</div>
  <div id="view-body"><nav id="dir-rail" aria-label="Project views"></nav><main id="root"></main></div>
  <div id="console-region"></div></div></div>`;

// Production ownership/rendering with the terminal transport and WASM replaced.
export const terminalShim = {
  name: "project-workspace-terminal-shim", enforce: "pre",
  resolveId(source, importer) {
    if (!importer?.includes("/src/")) return null;
    if (source.endsWith("/terminal/manager.js") || source === "./manager.js") return "\0project-terminal-manager";
    if (source.endsWith("/terminal/pane.js")) return "\0project-terminal-pane";
    return null;
  },
  load(id) {
    if (id === "\0project-terminal-manager") return `
      export const terminalManager=()=>window.__terminalManager;
      export const terminalDeviceId=()=>"console-device";
      export const subscribeTerminalStatus=(listener)=>{
        window.__terminalStatusListeners.add(listener);
        listener(window.__terminalStatus);
        return ()=>window.__terminalStatusListeners.delete(listener);
      };
      export const provideTerminalSessions=()=>{};
      export const followTerminalDevice=()=>{};
      export const resetTerminalManager=()=>{};
      export const createStatusHub=()=>({subscribe:()=>()=>{}});`;
    if (id !== "\0project-terminal-pane") return null;
    return `export async function mountTerminalPane(host, options) {
      const result=await options.attach({cols:80,rows:24,onSnapshot:()=>{},onOutput:()=>{},onClosed:()=>{}});
      const screen=document.createElement("pre");
      screen.className="project-terminal-screen"; screen.style.color="#c8d8dc"; screen.textContent=result.snapshot; host.appendChild(screen);
      return {dispose(){screen.remove();},focus(){screen.focus();}};
    }`;
  },
};

export async function mountProjectTerminal(page, basePath, { theme = "dark", supported = true, sourceCount = 2 } = {}) {
  page.setDefaultTimeout(5000);
  await mountLayout(page, SHELL, { basePath,
    styles: "#shell{--inbox-space:0px;height:100vh} #toolbar{padding:12px 20px} .project-terminal-screen{padding:16px;white-space:pre-wrap;color:#c8d8dc;font-size:13px}",
  });
  await loadBrowserModules(page, {
    app: "src/app.js", project: "src/views/projectView.js", shell: "src/core/shell.js",
    cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", contexts: "src/core/deviceContexts.js",
    changes: "src/core/changeEvents.js", adapter: "src/core/bridgeApi/v1/index.js", directory: "src/core/directoryScope.js",
  }, basePath);
  await page.evaluate(async ({ theme, supported, sourceCount }) => {
    document.documentElement.dataset.theme = theme;
    const { app, project, shell, cache, feed, contexts, changes, adapter, directory } = window.__layoutModules;
    const deviceId = "console-device";
    const sources = [{ id: "code", name: "Code", path: "/project/code", is_git: true }, { id: "notes", name: "Notes", path: "/project/notes", is_git: false }].slice(0, sourceCount);
    const record = { id: "p", project_id: "p", deviceId, projectKey: `${deviceId}/p`, name: "Build", path: "/project/code", entity_id: "project-owner", sources };
    const scopeOf = (source) => ({ project_id: "p", source_id: source.id, source_path: source.path });
    window.__terminalCalls = [];
    window.__terminalStatus = "connected";
    window.__terminalStatusListeners = new Set();
    window.__setTerminalStatus = (status) => {
      window.__terminalStatus = status;
      for (const listener of window.__terminalStatusListeners) listener(status);
    };
    const noteCall = (method, params) => window.__terminalCalls.push({ method, params });
    window.__terminalManager = {
      listTerminals: async (scope) => { noteCall("term.list", scope); return []; },
      createTerminal: async (scope) => { noteCall("term.create", scope); return { term_id: "term-created" }; },
      closeTerminal: async (termId, scope) => { noteCall("term.close", { termId, scope }); },
      attachTerminal: async (termId, scope) => { noteCall("term.attach", { termId, scope }); return { snapshot: `Shell in ${scope.source_path || "/project"}\n$ `, cursor: 0 }; },
      input: async () => {}, resize: async () => {}, detach() {},
    };
    const capabilities = ["fs.projectSources", "git.projectSources", "changes.projectSources"];
    if (supported) capabilities.push("term.projectSources");
    const greeting = { api_version: "3.21.0", capabilities };
    const call = async (method, params) => {
      noteCall(method, params);
      if (method === "session.hello") return greeting;
      if (method === "fs.tree") return { path: "", entries: [] };
      if (method === "git.status") return { staged: [], unstaged: [], untracked: [] };
      if (method === "git.log") return { commits: [] };
      return {};
    };
    const context = contexts.adoptDeviceSession({ deviceId, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    contexts.adoptBridgeSelection(context, { version: greeting.api_version }, adapter.create(call, greeting));
    await changes.greetBridge(call, { deviceId });
    app.App.devices = [{ id: deviceId, name: "Test device" }];
    app.App.route = { name: "project", deviceId, projectId: "p", tab: "workspaces" };
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [record]);
    for (const source of sources) await cache.writeCached({ deviceId, entityId: directory.directoryCacheId(scopeOf(source)), kind: "terminals" }, { tabs: [{ term_id: `term-${source.id}` }] });
    await feed.startFeed();
    shell.standShell(app.App.route);
    await project.renderProject();
  }, { theme, supported, sourceCount });
  // The rail mounts before the console's asynchronous cache reads paint tabs.
  await page.locator(".console-tab-name").first().waitFor();
}
