import { expect, it } from "vitest";
import { mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { shellHtml, SHELL_STYLES, settled } from "./chatMenuSeed.mjs";
import { expectNewHeadroom } from "./chatLandingAssertions.mjs";

async function seedWorkspaceLanding({ delayB = false, readB = false } = {}) {
  const { App } = window.__layoutModules.app;
  const { writeCached } = window.__layoutModules.cache;
  const { stampWorkspace } = window.__layoutModules.merge;
  const deviceId = "landing-device", projectId = "landing-project";
  App.devices = [{ id: deviceId, name: "Fixture computer" }];
  App.selectedDeviceId = deviceId;
  window.__layoutModules.contexts.knownDeviceContext(deviceId);
  const workspaces = ["a", "b"].map((id) => stampWorkspace({
    id: `workspace-${id}`, project_id: projectId, entity_id: `run-${id}`, name: `Workspace ${id}`,
  }, deviceId));
  const rows = {}, threads = {};
  window.__writeLandingRow = (id, patch = {}) => {
    rows[id] = { ...rows[id], agents: rows[id].agents.map((agent) => ({ ...agent, ...patch })) };
    return writeCached({ deviceId, entityId: `run-${id}`, kind: "row" }, rows[id]);
  };
  window.__writeLandingThread = (id, first = 1) => writeCached({
    deviceId, entityId: `run-${id}`, kind: "thread", sub: `agent-${id}`,
  }, { ...threads[id], items: threads[id].items.slice(first - 1), olderItemsRemain: first > 1 });
  for (const id of ["a", "b"]) {
    rows[id] = {
      kind: "workspace", entity_id: `run-${id}`, workspace_id: `workspace-${id}`, project_id: projectId,
      agents: [{ id: `agent-${id}`, name: `Agent ${id}`, ordinal: 1, provider: "codex_app_server",
        state: "idle", working: false, unread_count: id === "b" && readB ? 0 : 19,
        read_through_sequence: id === "b" && delayB ? undefined : 11 }],
    };
    threads[id] = {
      items: Array.from({ length: 30 }, (_, index) => ({ type: "message", data: {
        sequence: index + 1, role: "agent", body: `Workspace ${id} message ${index + 1}`,
      } })), olderItemsRemain: false,
    };
    await window.__writeLandingRow(id);
    await window.__writeLandingThread(id, id === "b" && delayB ? 20 : 1);
  }
  await writeCached({ deviceId, entityId: "", kind: "projects" }, [{ id: projectId, project_id: projectId, name: "Build" }]);
  await writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
  await window.__layoutModules.feed.startFeed();
  window.__workspaceRoute = (id) => ({ name: "workspace", deviceId, projectId, workspaceId: `workspace-${id}`, agent: `agent-${id}`, tab: "changes" });
  window.__openWorkspace = (id) => {
    App.route = window.__workspaceRoute(id);
    window.__layoutModules.shell.standShell(App.route);
  };
  window.__openWorkspace("a");
}

const linePosition = () => {
  const body = document.querySelector("#rail-body");
  const line = body.querySelector(".thread-unread-line");
  return { offset: line.getBoundingClientRect().top - body.getBoundingClientRect().top,
    top: body.scrollTop, height: body.clientHeight,
    headBottom: document.querySelector(".rail-head").getBoundingClientRect().bottom - body.getBoundingClientRect().top };
};

async function mountWorkspaceLanding(page, basePath, options) {
  await mountLayout(page, shellHtml("desktop"), { basePath, styles: `${SHELL_STYLES} .thread-message{min-height:160px}` });
  await loadChatOverviewModules(page, basePath, { shell: "src/core/shell.js", contexts: "src/core/deviceContexts.js", router: "src/core/router.js" });
  await page.evaluate(seedWorkspaceLanding, options);
  await page.waitForSelector(".thread-unread-line");
  await settled(page);
}

async function expectNewLanding(page) {
  await page.waitForSelector("#rail-body .thread-unread-line");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const position = await page.evaluate(linePosition);
  expectNewHeadroom(position.offset - position.headBottom, JSON.stringify(position));
}

it("lands both workspace switches on New through opening repaints", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWorkspaceLanding(page, basePath);
    await expectNewLanding(page); // cold opening and switches share the visible edge
    for (const id of ["b", "a"]) {
      await page.evaluate((id) => {
        const body = document.querySelector("#rail-body");
        body.scrollTop = id === "b" ? body.scrollHeight : 80;
        window.__openWorkspace(id);
      }, id);
      await page.waitForFunction((id) => document.querySelector(".thread-body")?.textContent.includes(`Workspace ${id}`), id);
      await settled(page);
      await expectNewLanding(page);
      for (let repaint = 0; repaint < 3; repaint += 1) {
        await page.evaluate(({ id, repaint }) => window.__writeLandingRow(id, { topic: `Repaint ${repaint}` }), { id, repaint });
        await page.waitForFunction((repaint) => document.querySelector(".rail-who")?.textContent.includes(`Repaint ${repaint}`), repaint);
        await expectNewLanding(page);
      }
    }
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  });
}, 30_000);

it("lands a switched conversation after its roster cursor and older history arrive", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWorkspaceLanding(page, basePath, { delayB: true });
    await page.evaluate(() => window.__openWorkspace("b"));
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b message 20"));
    await settled(page);
    expect(await page.locator(".thread-unread-line").count()).toBe(0);
    expect(await page.locator("#rail-body").evaluate((body) => body.scrollTop)).toBe(0);
    await page.evaluate(() => window.__writeLandingRow("b", { read_through_sequence: 11 }));
    await page.waitForSelector(".thread-unread-line");
    await page.evaluate(() => window.__writeLandingThread("b"));
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b message 1"), null, { timeout: 5000 });
    expect(await page.locator(".thread-unread-line").evaluate((line) => line.nextElementSibling.outerHTML)).toContain('data-sequence="12"');
    await settled(page);
    await expectNewLanding(page);
    await page.evaluate(() => window.__writeLandingThread("b"));
    await expectNewLanding(page);
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  });
}, 30_000);

it("keeps reader control during a switch's delayed opening", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWorkspaceLanding(page, basePath, { delayB: true });
    await page.evaluate(() => window.__openWorkspace("b"));
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b message 20"));
    await settled(page);
    await page.evaluate(() => {
      const body = document.querySelector("#rail-body");
      body.dispatchEvent(new WheelEvent("wheel"));
      body.scrollTop = 80;
      body.dispatchEvent(new Event("scroll"));
    });
    await page.evaluate(() => window.__writeLandingRow("b", { read_through_sequence: 21 }));
    await page.waitForSelector(".thread-unread-line");
    await settled(page);
    expect(await page.locator("#rail-body").evaluate((body) => body.scrollTop)).toBe(80);
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  });
}, 30_000);

it("lands a switched conversation with nothing unread at the end", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWorkspaceLanding(page, basePath, { readB: true });
    await page.evaluate(() => window.__openWorkspace("b"));
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b"));
    await settled(page);
    expect(await page.locator(".thread-unread-line").count()).toBe(0);
    expect(await page.locator("#rail-body").evaluate((body) => body.scrollHeight - body.clientHeight - body.scrollTop)).toBeLessThanOrEqual(1);
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  });
}, 30_000);

it("lands workspace links from the inbox and task surfaces and browser back", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountWorkspaceLanding(page, basePath);
    await page.evaluate(() => {
      const { App, initRouter } = window.__layoutModules.app;
      const { hashFromRoute } = window.__layoutModules.router;
      history.replaceState(null, "", hashFromRoute(window.__workspaceRoute("a")));
      App.gated = false;
      initRouter();
      const link = document.createElement("a");
      link.id = "inbox-workspace-link";
      link.href = hashFromRoute(window.__workspaceRoute("b"));
      link.textContent = "Workspace b";
      document.querySelector("#inbox-rail").append(link);
    });
    await page.locator("#inbox-workspace-link").click();
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b"));
    await expectNewLanding(page);
    await page.evaluate(() => {
      const link = document.createElement("a");
      link.id = "task-workspace-link";
      link.href = window.__layoutModules.router.hashFromRoute(window.__workspaceRoute("a"));
      link.textContent = "Assigned workspace";
      document.querySelector("#root").append(link);
    });
    await page.locator("#task-workspace-link").click();
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace a"));
    await expectNewLanding(page);
    await page.evaluate(() => { location.hash = window.__layoutModules.router.hashFromRoute(window.__workspaceRoute("b")); });
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace b"));
    await expectNewLanding(page);
    await page.goBack();
    await page.waitForFunction(() => document.querySelector(".thread-body")?.textContent.includes("Workspace a"));
    await expectNewLanding(page);
    await page.evaluate(() => window.__layoutModules.shell.stopShell());
  });
}, 30_000);
