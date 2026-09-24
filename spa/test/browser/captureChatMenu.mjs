// Capture the conversation head's ⋮ menu for #124, open, in both themes: at
// desktop and phone width, and at the narrowest phone width with four-digit
// counts on the surface rows. The production rail is mounted on a seeded
// workspace whose agent has surfaces and whose bridge carries compaction.
// Run from spa/: node test/browser/captureChatMenu.mjs before|after
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { mountLayout, withLayoutPage, loadBrowserModules } from "./layoutHarness.mjs";

const phase = process.argv[2] || "after";
const output = fileURLToPath(new URL("../../../design/chat-menu/", import.meta.url));
await mkdir(output, { recursive: true });

const shellHtml = (label) => `<div id="shell">${label === "desktop" ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Build / chat-menu-groups</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>chat-menu-groups</h1><p>Changes</p></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;
const styles = "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px;margin:0 0 8px}";

/** Each viewport: its size, whether a 2x close-up of the open menu is taken,
 *  and whether the surface rows carry four-digit counts (the wrap check). */
const VIEWPORTS = [
  ["desktop", { width: 1320, height: 850 }, { closeup: true, keyboard: true }],
  ["mobile", { width: 390, height: 844 }, {}],
  ["narrow", { width: 320, height: 640 }, { closeup: true, bigCounts: true }],
];
const THEMES = ["light", "dark"];

/** Runs in the page. */
async function seedChatMenu({ theme, bigCounts }) {
  document.documentElement.dataset.theme = theme;
  const { App } = window.__layoutModules.app;
  const { mountAgentRail } = window.__layoutModules.rail;
  const { writeCached } = window.__layoutModules.cache;
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
  await greetBridge(async () => ({ push_events: false, api_version: "1.10.0" }), { deviceId });
  await writeCached({ deviceId, entityId: "menu-run", kind: "row", sub: "" }, {
    kind: "workspace", entity_id: "menu-run", project_id: projectId, workspace_id: "menu-workspace", agents: [agent],
  });
  await writeCached({ deviceId, entityId: "menu-run", kind: "thread", sub: agent.id }, { items: [
    { type: "message", data: { id: "m1", sequence: 1, role: "user", body: "Separate the settings from what the agent opened.", created_at: minute(1) } },
    { type: "message", data: { id: "m2", sequence: 2, role: "agent", body: "Reading the menu code now.", created_at: minute(2) } },
  ] });
  await writeCached({ deviceId, entityId: "", kind: "projects", sub: "" }, [{ id: projectId, project_id: projectId, name: "Build" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces", sub: "" }, [
    { id: "menu-workspace", project_id: projectId, entity_id: "menu-run", name: "chat-menu-groups" },
  ].map((workspace) => stampWorkspace(workspace, deviceId)));
  await startFeed();
  window.__menuRail = mountAgentRail(document.querySelector("#agent-rail"), {
    kind: "workspace", deviceId, projectId, workspaceId: "menu-workspace", entityId: "menu-run",
    openAgentId: agent.id, panelOpen: true,
    call: async (method) => method === "models.list"
      ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
      : { items: [] },
  });
}

const settled = (page) => page.waitForFunction(() => document.getAnimations()
  .every((animation) => animation.effect?.getTiming().iterations === Infinity), null, { timeout: 5000 });

/** The rail on the page, its menu open and its motion settled. */
async function openMenuOn(page, basePath, label, seed) {
  await mountLayout(page, shellHtml(label), { basePath, styles });
  await loadBrowserModules(page, { app: "src/app.js" }, basePath);
  await page.evaluate(() => { window.__appModule = window.__layoutModules.app; delete window.__layoutModules; });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js",
    merge: "src/core/feedMerge.js", events: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(() => { window.__layoutModules.app = window.__appModule; });
  await page.evaluate(seedChatMenu, seed);
  await page.waitForSelector(".rail-surface-menu .caret", { timeout: 8000 });
  await page.waitForFunction(() => document.querySelectorAll('.rail-surface-menu .mi[data-action^="compact:"]').length > 0, null, { timeout: 8000 });
  await settled(page);
  await page.locator(".rail-surface-menu .caret").click();
  await page.waitForFunction(() => !document.querySelector(".rail-surface-menu .splitmenu").hidden, null, { timeout: 5000 });
  await settled(page);
}

/** The head and the open menu, with a margin, in page coordinates. */
const menuBox = (page) => page.evaluate(() => {
  const menu = document.querySelector(".rail-surface-menu .splitmenu").getBoundingClientRect();
  const head = document.querySelector(".rail-head").getBoundingClientRect();
  const x = Math.max(0, Math.min(menu.left, head.left) - 12);
  const y = Math.max(0, Math.min(menu.top, head.top) - 12);
  return { x, y, width: Math.min(window.innerWidth, Math.max(menu.right, head.right) + 12) - x, height: Math.min(window.innerHeight, Math.max(menu.bottom, head.bottom) + 12) - y };
});

for (const [label, viewport, { closeup = false, keyboard = false, bigCounts = false }] of VIEWPORTS) {
  for (const theme of THEMES) {
    await withLayoutPage(async ({ page, basePath }) => {
      const seed = { theme, bigCounts };
      await openMenuOn(page, basePath, label, seed);
      await page.screenshot({ path: `${output}${phase}-${theme}-${label}.png` });
      if (!closeup) return;
      // A close-up of the head and the open menu, so the rows read at review size.
      const box = await menuBox(page);
      const sharp = await page.context().browser().newPage({ viewport, deviceScaleFactor: 2 });
      await sharp.goto(page.url());
      await openMenuOn(sharp, basePath, label, seed);
      await sharp.screenshot({ path: `${output}${phase}-${theme}-${label}-closeup.png`, clip: box });
      if (keyboard && phase === "after") {
        // The keyboard's row: shut, reopen from the opener with the arrows,
        // walk two rows down, and show where focus is.
        await sharp.keyboard.press("Escape");
        await settled(sharp);
        await sharp.locator(".rail-surface-menu .caret").focus();
        await sharp.keyboard.press("ArrowDown");
        await sharp.keyboard.press("ArrowDown");
        await sharp.keyboard.press("ArrowDown");
        await settled(sharp);
        await sharp.screenshot({ path: `${output}${phase}-${theme}-menu-keyboard.png`, clip: box });
      }
      await sharp.close();
    }, viewport);
  }
}
console.log(`wrote ${output}`);
