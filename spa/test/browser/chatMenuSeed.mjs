// One workspace for the conversation head's ⋮ menu (#124) in a real Chromium:
// an agent with surfaces open, on a bridge that carries compaction, and the
// production rail mounted on it with nothing stood in. The capture script
// draws it (captureChatMenu.mjs); the keyboard check drives it
// (chatMenuKeyboardLayout.test.js).
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const shellHtml = (label) => `<div id="shell">${label === "desktop" ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Build / chat-menu-groups</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>chat-menu-groups</h1><p>Changes</p></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;
export const SHELL_STYLES = "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px;margin:0 0 8px}";

/** Runs in the page: `page.evaluate(seedChatMenu, { theme, bigCounts })`.
 *  `bigCounts` puts four-digit counts on the surface rows (the wrap check). */
export async function seedChatMenu({ theme, bigCounts, resetCapable = false, holdSettings = false, settingsDelayMs = 0 }) {
  document.documentElement.dataset.theme = theme;
  const { App } = window.__layoutModules.app;
  const { mountAgentRail } = window.__layoutModules.rail;
  const { readCached, writeCached } = window.__layoutModules.cache;
  const { startFeed } = window.__layoutModules.feed;
  const { stampWorkspace } = window.__layoutModules.merge;
  const { greetBridge } = window.__layoutModules.events;
  const deviceId = "menu-device";
  const projectId = "menu-project";
  App.devices = [{ id: deviceId, name: "This computer", status: "online" }];
  App.selectedDeviceId = deviceId;
  const minute = (at) => new Date(Date.UTC(2026, 8, 24, 1, at)).toISOString();
  const many = (count, make) => Array.from({ length: count }, (_, index) => make(index));
  const shells = bigCounts
    ? many(1024, (index) => ({ id: `sh${index}`, description: `npx vitest run ${index}`, state: "running", tail: [] }))
    : [{ id: "sh1", description: "npx vitest run", state: "running", tail: ["running 388 files"] }];
  const checklist = bigCounts
    ? many(5678, (index) => ({ id: `c${index}`, subject: `Step ${index}`, state: index < 1234 ? "completed" : "pending" }))
    : [
      { id: "c1", subject: "Pick the design", state: "completed" },
      { id: "c2", subject: "Group the menu", state: "in_progress" },
      { id: "c3", subject: "Screenshots", state: "pending" },
    ];
  const agent = {
    id: "menu-agent", name: "Chat menu design", topic: "Split chat context menu", ordinal: 1,
    provider: "claude_adk", state: "live", unread_count: 0, working: true, watched: true,
    conversation_id: "conversation-menu-agent",
    ...(resetCapable ? { thread_id: "thread:conversation-menu-agent", thread_generation_revision: 0 } : {}),
    last_context_tokens: 120000, max_context_tokens: null, compact_at_tokens: 200000,
    surface_session_generation: "session-one",
    surfaces: {
      shells,
      subagents: [
        { id: "s1", label: "screenshot runner", state: "done" },
        { id: "s2", label: "test writer", state: "running" },
      ],
      checklist,
    },
  };
  await greetBridge(async () => ({ push_events: false, api_version: "2.0.0", capabilities: ["changes.subscriptions", "requests.priority", "errors.codes", "diffs.perFile", "tasks.context", "tasks.attachments", "tasks.watching", "conversations.settings", ...(resetCapable ? ["conversation.reset"] : [])] }), { deviceId });
  await writeCached({ deviceId, entityId: "menu-run", kind: "row", sub: "" }, {
    kind: "workspace", entity_id: "menu-run", project_id: projectId, workspace_id: "menu-workspace", agents: [agent],
  });
  await writeCached({ deviceId, entityId: "menu-run", kind: "thread", sub: resetCapable ? agent.conversation_id : agent.id }, {
    ...(resetCapable ? { thread_id: agent.thread_id, thread_generation_revision: 0, deliveredSequence: 2 } : {}), items: [
    { type: "message", data: { id: "m1", sequence: 1, role: "user", body: "Separate the settings from what the agent opened.", created_at: minute(1) } },
    { type: "message", data: { id: "m2", sequence: 2, role: "agent", body: "Reading the menu code now.", created_at: minute(2) } },
  ] });
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" }, [{ id: projectId, project_id: projectId, name: "Build" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces", sub: "" }, [
    { id: "menu-workspace", project_id: projectId, entity_id: "menu-run", name: "chat-menu-groups" },
  ].map((workspace) => stampWorkspace(workspace, deviceId)));
  await startFeed();
  window.__menuSettingsAsked = [];
  window.__menuSettingsAnswered = [];
  window.__conversationResetCalls = [];
  window.__repaintMenuAgent = async (patch) => {
    const address = { deviceId, entityId: "menu-run", kind: "row", sub: "" };
    const row = (await readCached(address)).value;
    await writeCached(address, { ...row, agents: row.agents.map((held) => held.id === agent.id ? { ...held, ...patch } : held) });
  };
  window.__menuRail = mountAgentRail(document.querySelector("#agent-rail"), {
    kind: "workspace", deviceId, projectId, workspaceId: "menu-workspace", entityId: "menu-run",
    openAgentId: agent.id, panelOpen: true,
    call: async (method, params) => {
      if (method === "conversation.settings") {
        window.__menuSettingsAsked.push(params);
        if (holdSettings) await new Promise((resolve, reject) => {
          window.__releaseMenuSettings = resolve;
          window.__refuseMenuSettings = () => reject(new Error("settings refused"));
        });
        if (settingsDelayMs) await new Promise((resolve) => setTimeout(resolve, settingsDelayMs));
        window.__menuSettingsAnswered.push(params);
        return { agent_id: params.agent_id, max_context_tokens: params.max_context_tokens, compact_at_tokens: params.max_context_tokens ?? 200000 };
      }
      if (method === "models.list") return { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] };
      if (method === "conversation.reset" && resetCapable) {
        window.__conversationResetCalls.push(params);
        const threadId = "thread:conversation-menu-agent:browser-reset";
        return { entity_id: params.entity_id, agent_id: agent.id, conversation_id: agent.conversation_id,
          previous_thread_id: agent.thread_id, thread_id: threadId, thread_generation_revision: 1,
          agent: { ...agent, ...params, thread_id: threadId, thread_generation_revision: 1, working: false, state: "idle", topic: null, title: null,
            unread_count: 0, read_through_sequence: 0, start_error: null, last_context_tokens: null, last_context_at: null,
            session_cache_read_tokens: null, surfaces: null, choice_revision: 1 },
          thread: { thread_id: threadId, thread_generation_revision: 1, items: [], thread_total: 0, sessions: [] } };
      }
      return { items: [] };
    },
  });
}

/** Every animation on the page has ended, but for the ones that never do. */
export const settled = (page) => page.waitForFunction(() => document.getAnimations()
  .every((animation) => animation.effect?.getTiming().iterations === Infinity), null, { timeout: 5000 });

/** The rail on the page, its menu's controls painted and its motion settled.
 *  Keep the app module handle while the fixture loads the remaining modules. */
export async function mountChatMenu(page, basePath, label, seed) {
  await mountLayout(page, shellHtml(label), { basePath, styles: SHELL_STYLES });
  await loadBrowserModules(page, { app: "src/app.js" }, basePath);
  await page.evaluate(() => { window.__appModule = window.__layoutModules.app; delete window.__layoutModules; });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js",
    merge: "src/core/feedMerge.js", events: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(() => { window.__layoutModules.app = window.__appModule; });
  await page.evaluate(seedChatMenu, seed);
  await page.waitForSelector(".rail-surface-menu .caret", { timeout: 8000 });
  await page.waitForSelector('.rail-surface-menu [role="slider"]', { state: "attached", timeout: 8000 });
  await settled(page);
}

/** The rail on the page, its menu open from the pointer and its motion settled. */
export async function openMenuOn(page, basePath, label, seed) {
  await mountChatMenu(page, basePath, label, seed);
  await page.locator(".rail-surface-menu .caret").click();
  await page.waitForFunction(() => !document.querySelector(".rail-surface-menu .splitmenu").hidden, null, { timeout: 5000 });
  await settled(page);
}
