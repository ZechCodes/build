import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { SHELL_STYLES, settled } from "./chatMenuSeed.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

const WORKER = '[data-bubble="agent"][data-agent="project-worker"]';
const PIN = '[data-bubble="agent"][data-agent="project-pin"]';
const INPUT = ".thread-composer textarea";
const VIEWPORTS = [{ width: 390, height: 844 }, { width: 1280, height: 900 }];

// This fixture has no real bridge, pairing or account. The replica and the
// device repository are production objects; only RPC answers are stood in.
async function seedProjectWorkers() {
  const { app, cache, contexts, events, scope, feed, merge, rail } = window.__layoutModules;
  const deviceId = "workers-device";
  const projectId = "workers-project";
  const entityId = "project-owner";
  app.App.devices = [{ id: deviceId, name: "Fixture computer" }];
  app.App.selectedDeviceId = deviceId;
  app.App.route = { name: "project", deviceId, projectId, tab: "changes" };
  const agent = (id, name, projectRole) => ({
    id, name, topic: name, ordinal: projectRole === "coordinator" ? 1 : 2,
    project_role: projectRole, conversation_id: `conversation-${id}`, provider: "codex",
    state: "live", working: false, watched: true, unread_count: 0,
  });
  const pin = agent("project-pin", "Build coordinator", "coordinator");
  const worker = agent("project-worker", "Project worker", "worker");
  const own = agent("workspace-agent", "Workspace editor", null);
  window.__workerRoster = { entity_id: entityId, pinned_agent_id: pin.id, agents: [worker, pin] };
  window.__workerCalls = [];
  const catalog = { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] };
  window.__workerCall = async (method, params) => {
    window.__workerCalls.push({ method, params });
    if (method === "session.hello") return { api_version: "3.22.0", push_events: false,
      capabilities: window.__legacyWorkers ? [] : ["agents.projectWorkers", "tasks.assignProjectAgent"] };
    if (method === "models.list") return catalog;
    if (method === "agent.list") return window.__workerRoster;
    if (method === "agent.add") {
      const created = { ...agent("created-worker", params.name, "worker"), provider: params.provider };
      window.__workerRoster = { ...window.__workerRoster, agents: [...window.__workerRoster.agents, created] };
      return { entity_id: entityId, agent: created };
    }
    if (method === "thread.post") return { entity_id: params.entity_id, agent_id: params.agent_id,
      conversation_id: params.conversation_id, posted_sequence: 2 };
    if (method === "agent.start") return { agent_id: params.agent_id };
    return { items: [] };
  };
  await cache.writeCached({ deviceId, entityId: "", kind: "project-agents-support" },
    { projectWorkers: true, assignProjectAgent: true });
  await cache.writeCached({ deviceId, entityId, kind: "project-agents" }, window.__workerRoster);
  // Legacy rows and lists retain only the pin. The full roster is a separate
  // replica, intentionally ordered worker-first to expose first-agent aliases.
  await cache.writeCached({ deviceId, entityId, kind: "row" },
    { kind: "project", entity_id: entityId, project_id: projectId, agents: [pin] });
  await cache.writeCached({ deviceId, entityId: "", kind: "projects" },
    [merge.stampProject({ id: projectId, project_id: projectId, entity_id: entityId, name: "Build", agents: [pin] }, deviceId)]);
  await cache.writeCached({ deviceId, entityId: "workspace-owner", kind: "row" }, {
    kind: "workspace", entity_id: "workspace-owner", workspace_id: "workers-workspace", project_id: projectId, agents: [own],
  });
  await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [merge.stampWorkspace({
    id: "workers-workspace", project_id: projectId, entity_id: "workspace-owner", name: "Workspace",
  }, deviceId)]);
  for (const held of [pin, worker]) {
    await cache.writeCached({ deviceId, entityId, kind: "thread", sub: held.conversation_id }, { items: [
      { type: "message", data: { sequence: 1, role: "agent", body: `Cached ${held.name} conversation`,
        created_at: "2026-10-12T18:00:00Z" } },
    ] });
  }
  await feed.startFeed();
  const context = contexts.knownDeviceContext(deviceId);
  const dependencies = { cacheScope: scope.scopeFor(deviceId), chatRepository: context.chatRepository };
  const project = { kind: "project", deviceId, projectId, entityId, ...dependencies };
  window.__greetWorkers = async (legacy = false) => {
    window.__legacyWorkers = legacy;
    const adopted = contexts.adoptDeviceSession({ deviceId, call: window.__workerCall,
      close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await events.greetBridge(window.__workerCall, { deviceId, install: (selection) => {
      const adapter = selection.create(window.__workerCall);
      contexts.adoptBridgeSelection(adopted, selection, adapter);
      return adapter;
    } });
  };
  window.__workerRail = rail.mountAgentRail(document.querySelector("#agent-rail"), project);
}

async function mountWorkers(page, basePath) {
  page.setDefaultTimeout(5000);
  const desktop = page.viewportSize().width > 760;
  const markup = `<div id="shell">${desktop ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
    <header id="toolbar">Build / Project workspace</header><div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Build</h1><p>Project changes</p></main><aside id="agent-rail" aria-label="Agents"></aside>
    </div><div id="console-region"></div></div></div>`;
  await mountLayout(page, markup, { basePath, styles: SHELL_STYLES });
  await loadChatOverviewModules(page, basePath, {
    contexts: "src/core/deviceContexts.js", events: "src/core/changeEvents.js",
    scope: "src/core/cacheScope.js", shell: "src/core/shell.js",
  });
  await page.evaluate(seedProjectWorkers);
  await page.locator(WORKER).waitFor();
}

async function selectWithKeyboard(page, selector) {
  await page.locator(selector).focus();
  await page.keyboard.press("Enter");
  await settled(page);
  expect(await page.locator(selector).getAttribute("aria-expanded")).toBe("true");
}

for (const viewport of VIEWPORTS) {
  it(`paints cached workers before greeting, selects by keyboard and falls back on downgrade (${viewport.width})`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkers(page, basePath);
      const bubbles = await page.locator(".rail-strip > *").evaluateAll((nodes) => nodes.map((node) =>
        node.getAttribute("role") === "separator" ? "separator" : node.dataset.agent || node.dataset.bubble));
      expect(bubbles.slice(0, 4)).toEqual(["project-pin", "separator", "project-worker", "add"]);
      expect(await page.evaluate(() => window.__workerCalls)).toEqual([]);
      expect(await page.locator(PIN).evaluate((node) => node.classList.contains("active"))).toBe(true);
      await selectWithKeyboard(page, WORKER);
      await page.waitForFunction(() => document.querySelector(".thread-items")?.textContent.includes("Cached Project worker conversation"));
      await selectWithKeyboard(page, PIN);
      await page.waitForFunction(() => document.querySelector(".thread-items")?.textContent.includes("Cached Build coordinator conversation"));
      await selectWithKeyboard(page, WORKER);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(0);
      for (const theme of ["light", "dark"]) {
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await settled(page);
        await captureLayout(page, `536-${viewport.width}-${theme}.png`);
      }
      await page.evaluate(() => window.__greetWorkers(true));
      await page.locator(WORKER).waitFor({ state: "detached" });
      expect(await page.locator(PIN).count()).toBe(1);
      expect(await page.locator('[data-bubble="add"]').count()).toBe(0);
      expect(await page.evaluate(() => window.__workerCalls.some(({ params }) => params?.include_project_workers || params?.project_role))).toBe(false);
    }, viewport);
  }, 40_000);
}

it("names a worker through Add and sends only to each exact project conversation", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkers(page, basePath);
    await page.evaluate(() => window.__greetWorkers());
    await selectWithKeyboard(page, WORKER);
    await page.locator(INPUT).fill("Work on the project sources");
    await page.locator(".composer-send").click();
    await page.waitForFunction(() => window.__workerCalls.some(({ method }) => method === "thread.post"));
    const firstPost = await page.evaluate(() => window.__workerCalls.find(({ method }) => method === "thread.post").params);
    expect(firstPost).toMatchObject({ entity_id: "project-owner", agent_id: "project-worker",
      conversation_id: "conversation-project-worker", body: "Work on the project sources" });
    await selectWithKeyboard(page, '[data-bubble="add"]');
    expect(await page.locator(".rail-worker-notice").textContent()).toContain("Files, Git index and current branch are shared");
    await page.locator('.rail-harness-picker [data-provider="codex"]').click();
    await page.locator("[data-new-agent-name]").fill("Release helper");
    await page.locator(INPUT).fill("Check the release notes");
    await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
    await captureLayout(page, "536-add-390-light.png");
    await page.locator(".composer-send").click();
    await page.waitForFunction(() => window.__workerCalls.filter(({ method }) => method === "thread.post").length === 2).catch(async (error) => {
      const details = await page.evaluate(() => ({ calls: window.__workerCalls, text: document.body.textContent.slice(-300),
        bubbles: [...document.querySelectorAll("[data-bubble]")].map((node) => ({ agent: node.dataset.agent, type: node.dataset.bubble, active: node.classList.contains("active") })) }));
      throw new Error(`${error.message} ${JSON.stringify(details)}`);
    });
    const added = await page.evaluate(() => window.__workerCalls.find(({ method }) => method === "agent.add").params);
    expect(added).toMatchObject({ entity_id: "project-owner", project_role: "worker", name: "Release helper" });
    expect(added.creation_id).toEqual(expect.any(String));
    const createdPost = await page.evaluate(() => window.__workerCalls.filter(({ method }) => method === "thread.post")[1].params);
    expect(createdPost).toMatchObject({ entity_id: "project-owner", agent_id: "created-worker",
      conversation_id: "conversation-created-worker", body: "Check the release notes" });
    expect(createdPost.operation_id).not.toBe(firstPost.operation_id);
    await page.locator('[data-agent="created-worker"]').waitFor();
  }, { width: 390, height: 844 });
}, 40_000);

it("keeps project task workers and draft, while a named workspace shows its pin and own agents", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkers(page, basePath);
    await page.evaluate(() => {
      window.__workerRail.dispose();
      window.__layoutModules.shell.standShell(window.__layoutModules.app.App.route);
    });
    await page.locator(WORKER).waitFor();
    await selectWithKeyboard(page, WORKER);
    await page.locator(INPUT).fill("Keep this worker draft");
    const sameRail = await page.evaluate(() => {
      const { app, shell } = window.__layoutModules;
      const strip = document.querySelector(".rail-strip");
      const composer = document.querySelector(".thread-composer textarea");
      app.App.route = { ...app.App.route, name: "trackerTask", id: "project-task" };
      shell.standShell(app.App.route);
      return strip === document.querySelector(".rail-strip") && composer === document.querySelector(".thread-composer textarea");
    });
    expect(sameRail).toBe(true);
    expect(await page.locator(INPUT).inputValue()).toBe("Keep this worker draft");
    await selectWithKeyboard(page, PIN);
    await page.locator(INPUT).fill("Keep the coordinator draft across surfaces");
    await page.evaluate(() => {
      const { app, shell } = window.__layoutModules;
      app.App.route = { ...app.App.route, name: "workspace", workspaceId: "workers-workspace" };
      shell.standShell(app.App.route);
    });
    await page.locator('[data-agent="workspace-agent"]').waitFor();
    expect(await page.locator(WORKER).count()).toBe(0);
    expect(await page.locator('[data-bubble="project"]').count()).toBe(1);
    const sameWorkspaceRail = await page.evaluate(() => {
      const { app, shell } = window.__layoutModules;
      const strip = document.querySelector(".rail-strip");
      app.App.route = { ...app.App.route, taskId: "workspace-task" };
      shell.standShell(app.App.route);
      return strip === document.querySelector(".rail-strip");
    });
    expect(sameWorkspaceRail).toBe(true);
    expect(await page.locator(WORKER).count()).toBe(0);
    await selectWithKeyboard(page, '[data-bubble="project"]');
    expect(await page.locator(WORKER).count()).toBe(0);
    await page.waitForFunction(() => document.querySelector(".thread-items")?.textContent.includes("Cached Build coordinator conversation"));
    expect(await page.locator(INPUT).inputValue()).toBe("Keep the coordinator draft across surfaces");
    await page.evaluate(() => {
      const { app, shell } = window.__layoutModules;
      app.App.route = { name: "project", deviceId: "workers-device", projectId: "workers-project", tab: "changes" };
      shell.standShell(app.App.route);
    });
    await page.locator(WORKER).waitFor();
    await page.waitForFunction(() => document.querySelector(".thread-composer textarea")?.value === "Keep the coordinator draft across surfaces");
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  }, { width: 1280, height: 900 });
}, 40_000);
