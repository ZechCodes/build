// Cache-only Chromium evidence. Run unchanged against the baseline and fix
// with CAPTURE_PHASE=before|after; no bridge or application session is opened.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const phase = process.env.CAPTURE_PHASE || "after";
const directory = process.env.CAPTURE_DIR || "/tmp/task704-r2/captures";
assert.ok(["before", "after"].includes(phase), "CAPTURE_PHASE must be before or after");
await mkdir(directory, { recursive: true });

async function mountCapture(page, basePath) {
  let failedResponses = 0;
  const pending = new Set();
  page.on("request", (request) => pending.add(request.url()));
  page.on("requestfinished", (request) => pending.delete(request.url()));
  page.on("requestfailed", (request) => {
    pending.delete(request.url());
    console.error("Capture request failed:", request.failure(), request.url());
  });
  page.on("pageerror", (error) => console.error("Capture page error:", error));
  page.on("console", (message) => {
    if (message.type() === "error") console.error("Capture console:", message.text());
  });
  page.on("response", async (response) => {
    if (response.status() < 400 || failedResponses++ >= 5) return;
    const body = await response.text().catch(String);
    console.error("Capture response:", response.status(), response.url(), body.slice(0, 1000));
  });
  await mountLayout(page, `<div id="shell"><aside id="inbox-rail" aria-label="Inbox">
    <div class="inbox-head"><span class="dim">Inbox</span>
      <span id="capture-total" aria-label="Unread messages">0</span></div>
    <div id="inbox-list"></div></aside><main id="view">
      <section class="capture-chat"><h1>Build</h1><p class="dim">Project agent</p>
        <p>Ready for your review.</p><button id="reader-focus">Conversation open</button>
      </section></main></div>`, {
    basePath,
    styles: ".capture-chat { padding: 32px; } #capture-total { margin-left: auto; }",
  });
  await page.bringToFront();
  await page.locator("#reader-focus").focus();
  try {
    await loadBrowserModules(page, {
      rows: "src/core/inbox.js", projects: "src/core/inboxProjects.js",
      agents: "src/core/inboxProjectAgent.js", cache: "src/core/localCache.js",
      feed: "src/core/taskFeed.js", scopes: "src/core/syncVisibleScopes.js",
      attention: "src/core/inboxAttention.js",
    }, basePath);
  } catch (error) {
    console.error("Capture pending requests:", [...pending]);
    console.error("Capture page readiness:", await page.evaluate(() => ({
      url: location.href, readyState: document.readyState,
      focused: document.hasFocus(), hidden: document.hidden,
      fixturePresent: !!document.querySelector("#reader-focus"),
      moduleKeys: Object.keys(window.__layoutModules || {}),
      moduleError: window.__layoutModuleError,
    })));
    throw error;
  }
  await page.bringToFront();
  await page.locator("#reader-focus").focus();
}

async function seedCapture(page) {
  await page.evaluate(async () => {
    const { cache, feed, rows, agents, projects, scopes, attention } = window.__layoutModules;
    const deviceId = "open-unread-review", projectKey = `${deviceId}/build`;
    const project = { id: "build", name: "Build", entity_id: "owner", deviceId, projectKey };
    const identity = { deviceId, entityId: "owner", agentId: "pin",
      conversationId: "pin-history", threadId: "pin-thread" };
    const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
    const now = Date.now();
    const row = (unread) => ({
      kind: "branch", entity_id: identity.entityId, project_id: project.id, projectKey, deviceId,
      session_started_ms: now - 100_000, last_activity_ms: now,
      agents: [{ id: identity.agentId, conversation_id: identity.conversationId,
        thread_id: identity.threadId, topic: "Project coordinator", watched: true,
        unread_count: unread, read_through_sequence: 1, working: false }],
    });
    await cache.writeCached(cache.DEVICES_ADDRESS, [{ id: deviceId }]);
    await cache.writeCached(address("projects"), [project]);
    await cache.writeCached(address("agent-lineage-support"), { namesMakers: true });
    await cache.writeCached(address("row", identity.entityId), row(0));

    const capture = { trace: [], stage: "initial", current: null, running: true };
    window.__openUnread704 = capture;
    const record = (event) => {
      if (!capture.current) return;
      capture.trace.push({ event, stage: capture.stage, at: performance.now(),
        ...capture.current,
        dot: !!document.querySelector(`[data-key="project-agent:${projectKey}"] .inbox-status-unread`),
        shellTotal: Number(document.querySelector("#capture-total").textContent),
        focused: document.hasFocus(), visible: !document.hidden });
    };
    capture.stopTotal = attention.subscribeInboxAttentionCount((total) => {
      document.querySelector("#capture-total").textContent = String(total);
      record("shell-total");
    });
    // Subscribe Inbox first, as the mounted application does. A later read
    // response must not be able to conceal an earlier unread flash.
    capture.stopFeed = feed.subscribeFeed((view) => {
      const entries = [...agents.projectAgentEntries(view.projects, view.items, view.runs),
        ...rows.watchedWorkspaceEntries(view.workspaces, view.projects, view.items, view.runs)];
      document.querySelector("#inbox-list").innerHTML = entries.map((entry) => rows.inboxRowHtml(entry)).join("");
      const owner = view.items.find((item) => item.entity_id === identity.entityId);
      capture.current = {
        dot: !!document.querySelector(`[data-key="project-agent:${projectKey}"] .inbox-status-unread`),
        total: projects.projectsUnreadCount(entries, view.projects),
        rawUnread: owner?.agents.find((agent) => agent.id === identity.agentId)?.unread_count ?? 0,
      };
      attention.publishInboxAttentionCount(capture.current.total);
      record("inbox-paint");
    });
    capture.stopThread = scopes.registerSyncThread(() => identity, () => true);
    const frame = () => {
      if (!capture.running) return;
      record("animation-frame");
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
    await feed.startFeed();
    capture.arrive = async () => {
      capture.stage = "arrival";
      record("before-state");
      await cache.writeCached(address("row", identity.entityId), row(1));
      // State arrives before its transcript. No entity.seen call or reply is
      // involved, and the durable count deliberately remains unread.
      await cache.writeCached({ ...address("thread", identity.entityId), sub: identity.conversationId }, {
        thread_id: identity.threadId,
        items: [{ kind: "message", sequence: 2, role: "agent", body: "Ready for your review." }],
      });
    };
    capture.readRaw = async () => (await cache.readCached(address("row", identity.entityId)))
      .value.agents[0].unread_count;
    if (innerWidth <= 900) document.body.classList.add("inbox-collapsed", "inbox-popover-open");
  });
}

async function captureViewport(viewport) {
  return withLayoutPage(async ({ page, basePath }) => {
    await mountCapture(page, basePath);
    assert.equal(await page.evaluate(() => !document.hidden && document.hasFocus()), true,
      "the Chromium document must actually be visible and focused");
    await seedCapture(page);
    await page.evaluate(() => window.__openUnread704.arrive());
    await page.waitForFunction(() => window.__openUnread704.current?.rawUnread === 1);
    await page.waitForFunction(() => window.__openUnread704.trace.filter((reading) =>
      reading.event === "animation-frame" && reading.stage === "arrival").length >= 12,
    null, { timeout: 10_000 });
    await page.locator("#inbox-rail").screenshot({
      path: join(directory, `${phase}-open-unread-${viewport.label}.png`),
    });
    return page.evaluate(async () => {
      const capture = window.__openUnread704;
      capture.running = false;
      const evidence = { rawUnreadOnDisk: await capture.readRaw(),
        final: capture.current, trace: capture.trace };
      capture.stopFeed();
      capture.stopThread();
      capture.stopTotal();
      window.__layoutModules.feed.stopFeed();
      return evidence;
    });
  }, viewport);
}

function assertEvidence(evidence) {
  assert.equal(evidence.rawUnreadOnDisk, 1, "reader presence must preserve cached unread");
  const paints = evidence.trace.filter((reading) => reading.event === "inbox-paint");
  const frames = evidence.trace.filter((reading) => reading.event === "animation-frame");
  assert.ok(paints.length > 1, "the initial and arriving states both paint");
  assert.ok(frames.length >= 12, "each animation frame during the arrival is sampled");
  assert.ok(evidence.trace.every((reading) => reading.focused && reading.visible),
    "the reader remains present throughout the capture");
  if (phase === "before") {
    assert.ok(paints.some((reading) => reading.rawUnread === 1 && reading.dot && reading.total === 1));
    assert.ok(frames.some((reading) => reading.dot && reading.shellTotal === 1));
    assert.equal(evidence.final.total, 1);
  } else {
    assert.ok(evidence.trace.every((reading) => !reading.dot && reading.total === 0 && reading.shellTotal === 0),
      "no synchronous paint, published total or animation frame may flash unread");
  }
}

const observations = {};
for (const viewport of [
  { label: "desktop", width: 1280, height: 800 },
  { label: "phone", width: 390, height: 760, hasTouch: true },
]) {
  observations[viewport.label] = await captureViewport(viewport);
}
await writeFile(join(directory, `${phase}-open-unread.json`), `${JSON.stringify(observations, null, 2)}\n`);
Object.values(observations).forEach(assertEvidence);
console.log(JSON.stringify({ phase, observations }));
