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
    scope: "src/core/cacheScope.js", shell: "src/core/shell.js", viewing: "src/core/viewingContext.js",
    router: "src/core/router.js",
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

for (const { state, news } of [
  { state: "done", news: "The agent finished — review the work" },
  { state: "blocked", news: "Blocked — the agent needs you" },
  { state: "working", news: "working" },
]) {
  it(`announces the project worker's unread badge in its accessible name (${state})`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkers(page, basePath);
      await page.evaluate(async (state) => {
        const { cache } = window.__layoutModules;
        window.__workerRoster.agents = window.__workerRoster.agents.map((agent) => ({ ...agent,
          unread_count: 2, unread_reason: state === "working" ? "done" : state,
          working: agent.id === "project-worker" && state === "working",
        }));
        await cache.writeCached({ deviceId: "workers-device", entityId: "project-owner", kind: "project-agents" },
          window.__workerRoster);
      }, state);
      await page.locator(`${WORKER} .rail-count:not([hidden])`).waitFor();
      expect(await page.locator(`${WORKER} .rail-count`).textContent()).toBe("2");
      const title = `Project worker · Project worker — ${news}`;
      expect(await page.locator(WORKER).getAttribute("title")).toBe(title);
      expect(await page.getByRole("button", { name: `${title}, 2 unread`, exact: true }).count()).toBe(1);
      expect(await page.getByRole("button", { name: "Project agent, 2 unread", exact: true }).count()).toBe(1);
      expect(await page.locator(PIN).getAttribute("title")).toBe("Project agent");
      await page.evaluate(async () => {
        const { cache } = window.__layoutModules;
        window.__workerRoster.agents = window.__workerRoster.agents.map((agent) => ({ ...agent, unread_count: 0 }));
        await cache.writeCached({ deviceId: "workers-device", entityId: "project-owner", kind: "project-agents" },
          window.__workerRoster);
      });
      await page.locator(`${WORKER} .rail-count`).waitFor({ state: "hidden" });
      const readTitle = `Project worker · Project worker${state === "working" ? " — working" : ""}`;
      expect(await page.getByRole("button", { name: readTitle, exact: true }).count()).toBe(1);
      expect(await page.getByRole("button", { name: "Project agent", exact: true }).count()).toBe(1);
      expect(await page.getByRole("button", { name: /unread/ }).count()).toBe(0);
    });
  });
}

it("omits unread from accessible names when the worker and pin have no badge", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkers(page, basePath);
    expect(await page.getByRole("button", { name: "Project worker · Project worker", exact: true }).count()).toBe(1);
    expect(await page.getByRole("button", { name: "Project agent", exact: true }).count()).toBe(1);
    expect(await page.getByRole("button", { name: /unread/ }).count()).toBe(0);
    expect(await page.locator(`${WORKER} .rail-count`).isHidden()).toBe(true);
    expect(await page.locator(`${PIN} .rail-count`).isHidden()).toBe(true);
  });
});

it("announces a workspace agent's unread badge through the same accessible name", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkers(page, basePath);
    await page.evaluate(async () => {
      const { cache, contexts, rail, scope } = window.__layoutModules;
      const address = { deviceId: "workers-device", entityId: "workspace-owner", kind: "row" };
      const row = (await cache.readCached(address)).value;
      await cache.writeCached(address, { ...row, agents: row.agents.map((agent) => ({ ...agent,
        unread_count: 2, unread_reason: "blocked",
      })) });
      window.__workerRail.dispose();
      window.__workerRail = rail.mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "workspace", deviceId: "workers-device", projectId: "workers-project",
        workspaceId: "workers-workspace", entityId: "workspace-owner",
        cacheScope: scope.scopeFor("workers-device"),
        chatRepository: contexts.knownDeviceContext("workers-device").chatRepository,
      });
    });
    const selector = '[data-bubble="agent"][data-agent="workspace-agent"]';
    await page.locator(`${selector} .rail-count:not([hidden])`).waitFor();
    expect(await page.locator(`${selector} .rail-count`).textContent()).toBe("2");
    const title = "Workspace editor · Workspace editor — Blocked — the agent needs you";
    expect(await page.locator(selector).getAttribute("title")).toBe(title);
    expect(await page.getByRole("button", { name: `${title}, 2 unread`, exact: true }).count()).toBe(1);
  });
});

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


for (const viewport of VIEWPORTS) {
  it(`keeps the exact focused pin conversation mounted throughout native hash host changes (${viewport.width})`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkers(page, basePath);
      await page.evaluate(async () => {
        const { app, cache, merge, shell, viewing } = window.__layoutModules;
        app.App.viewingContext = viewing.createViewingContext();
        await cache.writeCached({ deviceId: "workers-device", entityId: "workspace-b-owner", kind: "row" }, {
          kind: "workspace", entity_id: "workspace-b-owner", workspace_id: "workspace-b", project_id: "workers-project",
          agents: [{ id: "workspace-b-agent", name: "B editor", state: "live", provider: "codex" }],
        });
        const address = { deviceId: "workers-device", entityId: "", kind: "workspaces" };
        const rows = (await cache.readCached(address)).value;
        await cache.writeCached(address, [...rows, merge.stampWorkspace({ id: "workspace-b", project_id: "workers-project",
          entity_id: "workspace-b-owner", name: "Workspace B" }, "workers-device")]);
        window.__workerRail.dispose();
        app.App.route.agent = "project-pin";
        history.replaceState(null, "", window.__layoutModules.router.hashFromRoute(app.App.route));
        shell.standShell(app.App.route);
      });
      await page.waitForFunction(() => document.querySelector(".thread-items")?.textContent.includes("Cached Build coordinator"));
      await page.locator(INPUT).fill("Keep my exact mounted draft");
      await page.locator(INPUT).focus();
      await page.evaluate(() => {
        const selectors = [".thread-composer textarea", "#rail-body", ".thread-items"];
        const held = selectors.map((selector) => document.querySelector(selector));
        const failures = [];
        window.__continuity = { failures, samples: 0 };
        const sample = (phase) => {
          window.__continuity.samples++;
          held.forEach((node, index) => {
            const rect = node.getBoundingClientRect();
            if (!node.isConnected || document.querySelector(selectors[index]) !== node || !rect.width || !rect.height ||
              !node.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }))
              failures.push(`${phase}: ${selectors[index]} detached, replaced or hidden`);
          });
          if (document.activeElement !== held[0]) failures.push(`${phase}: composer focus lost`);
        };
        const observer = new MutationObserver((records) => {
          for (const record of records) {
            for (const removed of record.removedNodes)
              if (held.some((node) => removed === node || removed.contains?.(node))) failures.push("mutation: held conversation removed");
            if (!record.target.contains?.(held[0])) continue;
            const wasHidden = record.attributeName === "aria-hidden" && record.oldValue === "true"
              || record.attributeName === "inert" && record.oldValue !== null
              || record.attributeName === "class" && record.oldValue?.split(" ").includes("rail-collapsed");
            if (wasHidden) failures.push("mutation: held conversation temporarily hidden");
          }
          sample("mutation");
        });
        observer.observe(document.querySelector("#agent-rail"), { subtree: true, childList: true, attributes: true, attributeOldValue: true });
        const focusout = () => failures.push("focusout: composer blurred");
        held[0].addEventListener("focusout", focusout);
        let running = true;
        const frame = () => { if (running) { sample("frame"); requestAnimationFrame(frame); } };
        requestAnimationFrame(frame);
        window.__continuity.sample = sample;
        window.__continuity.stop = () => { running = false; observer.disconnect(); held[0].removeEventListener("focusout", focusout); };
      });
      for (const step of [
        { name: "workspace", workspaceId: "workers-workspace", own: "workspace-agent", stamp: "from Workspace" },
        { name: "workspace", workspaceId: "workspace-b", own: "workspace-b-agent", stamp: "from Workspace B" },
        { name: "project", own: "project-worker", stamp: null },
      ]) {
        await page.evaluate(async (step) => {
          const { app, router, shell } = window.__layoutModules;
          const destination = { name: step.name, workspaceId: step.workspaceId, deviceId: "workers-device",
            projectId: "workers-project", agent: "project-pin", tab: "changes" };
          const changed = new Promise((resolve) => window.addEventListener("hashchange", resolve, { once: true }));
          location.hash = router.hashFromRoute(destination);
          window.__continuity.sample("synchronous hash assignment");
          await changed;
          // The app router awaits navigation guards before standing the shell.
          // Sample the native event and a frame before that destination render.
          window.__continuity.sample("native hashchange");
          await new Promise(requestAnimationFrame);
          window.__continuity.sample("before destination render");
          app.App.route = router.routeFromHash(location.hash);
          shell.standShell(app.App.route);
          window.__continuity.sample("synchronous destination render");
        }, step);
        await page.locator(`[data-agent="${step.own}"]`).waitFor();
        await settled(page);
        if (step.stamp) {
          expect(await page.locator('.thread-composer [data-context-kind="workspace"]').textContent()).toBe(step.stamp);
          expect(await page.locator(WORKER).count()).toBe(0);
        } else expect(await page.locator('.thread-composer [data-context-kind="workspace"]').count()).toBe(0);
        expect(await page.locator(INPUT).inputValue()).toBe("Keep my exact mounted draft");
        expect(await page.evaluate(() => window.__continuity.failures)).toEqual([]);
      }
      const state = await page.evaluate(() => { window.__continuity.stop(); return window.__continuity; });
      expect(state.samples).toBeGreaterThan(6);
      expect(state.failures).toEqual([]);
      await page.evaluate(() => window.__greetWorkers());
      await page.locator(".composer-send").click();
      await page.waitForFunction(() => window.__workerCalls.some(({ method }) => method === "thread.post"));
      const posted = await page.evaluate(() => window.__workerCalls.find(({ method }) => method === "thread.post").params);
      expect(posted).toMatchObject({ entity_id: "project-owner", agent_id: "project-pin", conversation_id: "conversation-project-pin",
        body: "Keep my exact mounted draft" });
      expect(posted.viewing_context?.items?.some((item) => item.kind === "workspace") || false).toBe(false);
      await page.evaluate(() => window.__layoutModules.shell.stopShell());
    }, viewport);
  }, 40_000);
}


it("refuses continuity for workers, unknown identity and unrelated navigation", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWorkers(page, basePath);
    await page.locator(INPUT).waitFor();
    const decisions = await page.evaluate(() => {
      const { contexts, scope } = window.__layoutModules;
      const context = contexts.knownDeviceContext("workers-device");
      const descriptor = { kind: "workspace", workspaceId: "workers-workspace", deviceId: "workers-device",
        projectId: "workers-project", openAgentId: "project-pin", projectAgent: { projectId: "workers-project" },
        cacheScope: scope.scopeFor("workers-device"), chatRepository: context.chatRepository };
      const textarea = document.querySelector(".thread-composer textarea");
      const results = [
        { openAgentId: null }, { openAgentId: "project-worker" }, { deviceId: "other-device" },
        { projectId: "other-project" }, { chatRepository: {} }, { kind: "branch" },
      ].map((change) => window.__workerRail.retarget({ ...descriptor, ...change }));
      return { results, same: textarea === document.querySelector(".thread-composer textarea") };
    });
    expect(decisions).toEqual({ results: [false, false, false, false, false, false], same: true });
    await selectWithKeyboard(page, WORKER);
    expect(await page.evaluate(() => window.__workerRail.retarget({ kind: "project", deviceId: "workers-device",
      projectId: "workers-project", openAgentId: "project-worker",
      chatRepository: window.__layoutModules.contexts.knownDeviceContext("workers-device").chatRepository }))).toBe(false);
    const cold = await page.evaluate(() => {
      const { rail, contexts } = window.__layoutModules;
      const host = document.createElement("aside"); document.body.append(host);
      const repository = contexts.knownDeviceContext("workers-device").chatRepository;
      const mounted = rail.mountAgentRail(host, { kind: "project", deviceId: "workers-device", projectId: "unknown-project",
        entityId: "unknown-owner", openAgentId: "unknown-pin", chatRepository: repository });
      const kept = mounted.retarget({ kind: "project", deviceId: "workers-device", projectId: "unknown-project",
        openAgentId: "unknown-pin", chatRepository: repository });
      mounted.dispose(); host.remove(); return kept;
    });
    expect(cold).toBe(false);
    await page.evaluate(() => window.__workerRail.dispose());
  }, { width: 1280, height: 900 });
}, 40_000);


it("dismisses the default mobile panel when native navigation leaves the canonical pin", async () => {
  for (const destination of [
    { agent: "project-worker" }, { agent: null }, { agent: null, newAgent: true },
    { projectId: "other-project" }, { deviceId: "other-device" }, { name: "inbox" },
  ]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountWorkers(page, basePath);
      await page.evaluate(() => {
        const { app, router, shell } = window.__layoutModules;
        window.__workerRail.dispose();
        app.App.route.agent = "project-pin";
        history.replaceState(null, "", router.hashFromRoute(app.App.route));
        shell.standShell(app.App.route);
      });
      await page.locator(INPUT).fill("Draft on the pin");
      await page.locator(INPUT).focus();
      expect(await page.locator(".rail-head .pinbtn").getAttribute("aria-pressed")).toBe("false");
      await page.evaluate(async (destination) => {
        const { app, router } = window.__layoutModules;
        const changed = new Promise((resolve) => window.addEventListener("hashchange", resolve, { once: true }));
        location.hash = router.hashFromRoute({ ...app.App.route, ...destination });
        await changed;
      }, destination);
      await settled(page);
      expect(await page.locator(INPUT).isVisible(), JSON.stringify(destination)).toBe(false);
      expect(await page.locator(INPUT).evaluate((node) => document.activeElement === node)).toBe(false);
      await page.evaluate(() => window.__layoutModules.shell.stopShell());
    }, { width: 390, height: 844 });
  }
}, 40_000);
