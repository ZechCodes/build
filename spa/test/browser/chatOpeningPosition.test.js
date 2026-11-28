import { readFileSync } from "node:fs";
import { afterAll, beforeAll, expect, it } from "vitest";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { expectNewHeadroom } from "./chatLandingAssertions.mjs";
import { mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const shell = readFileSync(new URL("../../index.html", import.meta.url), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const viewport = { width: 1180, height: 840 };
const firstUnread = 12;
let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

async function loadShell(page, basePath) {
  await mountLayout(page, shell, { basePath, styles: ".thread-message{min-height:160px}" });
  await loadChatOverviewModules(page, basePath, {
    contexts: "src/core/deviceContexts.js", inbox: "src/core/inboxView.js", router: "src/core/router.js",
  });
}

async function seedChat(readThrough) {
  const { cache } = window.__layoutModules;
  const deviceId = "opening-device", projectId = "opening-project", entityId = "project-run";
  const now = new Date().toISOString();
  const agent = { id: "coordinator", name: "Build", topic: "Chat landing", ordinal: 1, provider: "codex",
    watched: true, unread_count: 30 - readThrough, read_through_sequence: readThrough };
  const row = { kind: "project", entity_id: entityId, project_id: projectId, pinned_agent_id: agent.id,
    anchor: now, last_activity: now, agents: [agent] };
  const address = { deviceId, entityId, kind: "thread", sub: agent.id };
  const message = (sequence, body = `Chat message ${sequence}`) => ({ type: "message", data: { sequence, role: "agent", body } });
  const items = Array.from({ length: 29 }, (_, index) => message(index + 2));
  await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId, name: "Cached computer" }]);
  await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [{ id: projectId, project_id: projectId,
    deviceId, projectKey: `${deviceId}/${projectId}`, name: "Build", entity_id: entityId }]);
  await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [], runs: [row] });
  await cache.writeCached({ deviceId, entityId, kind: "row" }, row);
  await cache.writeCached(address, { items, deliveredSequence: 30 });
  window.__landChatPull = async () => {
    items.unshift(message(1, "Older page\n\n```js\n" + "const value = 1;\n".repeat(42) + "```"));
    items.push(message(31, "Newer page"));
    await cache.writeCached(address, { items, deliveredSequence: 31 });
  };
  window.__landChatPush = async () => {
    items.push(message(32, "New push"));
    await cache.writeCached(address, { items, deliveredSequence: 32 });
  };
}

async function startCachedShell(hash) {
  const { app, cache, contexts, feed, inbox } = window.__layoutModules;
  app.App.devices = (await cache.readCached(cache.DEVICES_ADDRESS)).value;
  app.App.selectedDeviceId = app.App.devices[0].id;
  contexts.knownDeviceContext(app.App.selectedDeviceId);
  app.App.gated = false;
  inbox.mountInboxList();
  await feed.startFeed();
  if (hash) history.replaceState(null, "", hash);
  app.initRouter();
  app.render();
}

// Start sampling before navigation. Box geometry comes from the first visible
// frame with transcript rows, even when opening and cache reads span frames.
async function recordFirstTranscriptFrame(page) {
  await page.evaluate(() => {
    window.__firstChatFrame = new Promise((resolve) => {
      const sample = () => {
        const scroller = document.querySelector("#rail-body");
        const panel = document.querySelector("#rail-panel");
        const last = scroller?.querySelector('.thread-message[data-sequence="30"]');
        if (!scroller?.querySelector(".thread-message") || getComputedStyle(panel).visibility === "hidden" || Number(getComputedStyle(panel).opacity) === 0) {
          requestAnimationFrame(sample);
          return;
        }
        const viewport = scroller.getBoundingClientRect();
        const head = panel.querySelector(".rail-head").getBoundingClientRect();
        const top = Math.max(viewport.top, head.bottom);
        const line = scroller.querySelector(".thread-unread-line")?.getBoundingClientRect();
        const newest = last?.getBoundingClientRect();
        resolve({
          scrollTop: scroller.scrollTop,
          bottomGap: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
          lineVisible: Boolean(line && line.top >= top && line.bottom <= viewport.bottom),
          newestVisible: Boolean(newest && newest.top >= top && newest.bottom <= viewport.bottom),
        });
      };
      requestAnimationFrame(sample);
    });
  });
}

const projectHash = "#/device/opening-device/project/opening-project";
const chatHash = `${projectHash}?agent=coordinator`;
const frames = (page) => page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));

async function currentPosition(page) {
  return page.locator("#rail-body").evaluate((scroller) => {
    const viewport = scroller.getBoundingClientRect();
    const head = document.querySelector(".rail-head").getBoundingClientRect();
    const line = scroller.querySelector(".thread-unread-line")?.getBoundingClientRect();
    return { lineOffset: line && line.top - Math.max(viewport.top, head.bottom),
      bottomGap: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
      scrollTop: scroller.scrollTop };
  });
}

async function growCachedImage(page, basePath, unread) {
  await page.route("**/opening-image.svg", (route) => route.fulfill({ contentType: "image/svg+xml",
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="80" height="760"><rect width="80" height="760"/></svg>' }));
  await page.evaluate(async ({ src, unread }) => {
    const sequence = unread ? 8 : 28;
    const message = document.querySelector(`#rail-body .thread-message[data-sequence="${sequence}"] .thread-body`);
    const image = new Image();
    message.append(image);
    image.src = src;
    await image.decode();
  }, { src: `${basePath}opening-image.svg`, unread });
  await frames(page);
}

async function expectLanding(page, unread) {
  const position = await currentPosition(page);
  if (unread) expectNewHeadroom(position.lineOffset, JSON.stringify(position));
  else expect(position.bottomGap).toBeLessThanOrEqual(1);
  return position.scrollTop;
}

it.each([
  { route: "inbox", unread: true },
  { route: "inbox", unread: false },
  { route: "project face", unread: true },
  { route: "project face", unread: false },
  { route: "direct reload", unread: true },
  { route: "direct reload", unread: false },
])("lands cached chat on its first visible frame via $route (unread=$unread) and holds through late layout", async ({ route, unread }) => {
  await session.withPage(async ({ page, basePath }) => {
    page.setDefaultTimeout(5000);
    await loadShell(page, basePath);
    await page.evaluate(seedChat, unread ? firstUnread - 1 : 30);
    if (route === "direct reload") {
      await page.evaluate((hash) => history.replaceState(null, "", hash), chatHash);
      await page.reload();
      await loadShell(page, basePath);
    }
    if (route === "project face") await page.evaluate(startCachedShell, projectHash);
    if (route === "inbox") await page.evaluate(startCachedShell, "#/inbox");
    await recordFirstTranscriptFrame(page);
    if (route === "direct reload") await page.evaluate(startCachedShell);
    if (route === "project face") await page.locator('[data-agent="coordinator"]').click();
    if (route === "inbox") await page.locator(".inbox-project-agent").click();
    await page.waitForSelector('#rail-body .thread-message[data-sequence="30"]');
    const first = await page.evaluate(() => window.__firstChatFrame);
    expect(first.scrollTop).toBeGreaterThan(0);
    if (unread) expect(first.lineVisible).toBe(true);
    else {
      expect(first.bottomGap).toBeLessThanOrEqual(1);
      expect(first.newestVisible).toBe(true);
    }

    await frames(page);
    await growCachedImage(page, basePath, unread);
    await expectLanding(page, unread);
    if (route === "direct reload") return;

    await page.evaluate(() => window.__landChatPull());
    await page.waitForSelector('#rail-body .thread-message[data-sequence="31"]');
    const landedTop = await expectLanding(page, unread);
    const body = page.locator("#rail-body");
    await body.hover();
    await page.mouse.wheel(0, -400);
    await page.waitForFunction((top) => document.querySelector("#rail-body").scrollTop < top, landedTop);
    await frames(page);
    const readerTop = (await currentPosition(page)).scrollTop;
    await page.evaluate(() => window.__landChatPush());
    await page.waitForSelector('#rail-body .thread-message[data-sequence="32"]');
    await frames(page);
    expect((await currentPosition(page)).scrollTop).toBe(readerTop);
  }, viewport);
}, 30_000);
