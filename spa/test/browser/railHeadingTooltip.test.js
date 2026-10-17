import assert from "node:assert/strict";
import { afterAll, beforeAll, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession(); }, 30_000);
afterAll(async () => { await session?.close(); });

const markup = (desktop) => `<div id="shell">${desktop ? '<aside id="inbox-rail"></aside>' : ""}<div id="view">
  <header id="toolbar">Build / Workspace</header><div id="view-body">
    <main id="root">Workspace</main><aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;

async function mountRail(page, basePath) {
  await mountLayout(page, markup(page.viewportSize().width > 600), { basePath });
  await loadBrowserModules(page, { rail: "src/core/agentRail.js", cache: "src/core/localCache.js" }, basePath);
  await page.evaluate(async () => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    window.__railRow = {
      run_id: "layout-run", project_id: "layout-project", agents: [{
        id: "layout-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Rail helper", topic: "Old work", model: "claude-opus-5", watched: true,
        working: false, unread_count: 0,
      }],
    };
    window.__writeRailRow = (row) => writeCached({ deviceId: "layout-device", entityId: "layout-run", kind: "row", sub: "" }, row);
    await window.__writeRailRow(window.__railRow);
    window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: "layout-device", projectId: "layout-project", entityId: "layout-run",
      openAgentId: "layout-agent", panelOpen: true,
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  });
  await page.waitForSelector(".rail-who-work");
}

for (const width of [1180, 390]) it(`keeps the ${width}px header and controls still through every erased and typed frame`, async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountRail(page, basePath);
    const samples = await page.evaluate(async () => {
      const head = document.querySelector(".rail-head");
      const work = head.querySelector(".rail-who-work");
      const rect = (node) => {
        const box = node.getBoundingClientRect();
        return { x: box.x, y: box.y, width: box.width, height: box.height };
      };
      const unobscured = (node) => {
        const box = node.getBoundingClientRect();
        return head.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
      };
      const sample = () => ({ text: work.textContent, nameText: head.querySelector(".rail-who-name").textContent,
        name: rect(head.querySelector(".rail-who-name")), harness: rect(head.querySelector(".rail-harness-icon")),
        model: rect(head.querySelector(".agent-model-badge")), watch: rect(head.querySelector(".rail-watch")),
        head: rect(head), work: rect(work),
        nameUnobscured: unobscured(head.querySelector(".rail-who-name")), workUnobscured: unobscured(work) });
      const seen = [sample()];
      const observer = new MutationObserver(() => seen.push(sample()));
      observer.observe(work, { childList: true, characterData: true, subtree: true });
      await window.__writeRailRow({ ...window.__railRow, agents: [{ ...window.__railRow.agents[0], topic: "Updated work" }] });
      await new Promise((resolve, reject) => {
        const started = Date.now();
        const poll = () => {
          if (work.textContent === "Updated work" && !work.dataset.titleMotion) { resolve(); return; }
          if (Date.now() - started > 3000) { reject(new Error("Header motion did not settle")); return; }
          setTimeout(poll, 10);
        };
        poll();
      });
      observer.disconnect();
      return seen;
    });
    assert.ok(samples.some((frame) => frame.text === ""), "the erased frame was measured");
    for (const frame of samples) {
      assert.equal(frame.nameText, "Rail helper", "the agent name never animates");
      assert.equal(frame.nameUnobscured, true, "the name must be visible above the page");
      assert.equal(frame.workUnobscured, true, "the work must be visible above the page");
      for (const key of ["name", "harness", "model", "watch", "head"]) {
        for (const dimension of ["x", "y", "width", "height"]) {
          assert.ok(Math.abs(frame[key][dimension] - samples[0][key][dimension]) <= 1, `${key}.${dimension} shifted`);
        }
      }
      assert.ok(frame.work.height >= 14, "work line collapsed while empty");
    }
    await captureLayout(page, `rail-two-line-header-${width}.png`);
    await page.evaluate(async () => {
      await window.__writeRailRow({ ...window.__railRow, agents: [{ ...window.__railRow.agents[0], topic: "" }] });
    });
    await page.waitForSelector(".rail-who-work", { state: "detached" });
    assert.equal(await page.locator(".rail-who-name").textContent(), "Rail helper");
    assert.equal(await page.locator(".rail-who").count(), 1);
    assert.equal(await page.locator(".rail-who").evaluate((who) => who.children.length), 1);
    await captureLayout(page, `rail-name-only-header-${width}.png`);
  }, { width, height: 840 });
}, 30_000);

async function mountBubble(page, basePath) {
  await mountLayout(page, '<div id="bubble-fixture"></div>', { basePath });
  await loadBrowserModules(page, {
    rail: "src/core/agentRail.js", tooltip: "src/core/railBubbleTooltip.js",
  }, basePath);
  await page.evaluate(() => {
    const { bubbleHtml } = window.__layoutModules.rail;
    const { createRailBubbleTooltip } = window.__layoutModules.tooltip;
    document.querySelector("#bubble-fixture").innerHTML = bubbleHtml({
      type: "agent", id: "layout-agent", title: "Rail helper", label: "R", unread: 0,
      tooltip: { name: "Rail helper", work: "Fix the conversation header", status: "idle",
        provider: "Claude Code", model: "Claude Opus 5" },
    });
    const button = document.querySelector(".rail-bubble");
    button.style.position = "fixed";
    button.style.right = "2px";
    button.style.bottom = "2px";
    window.__bubbleClicks = 0;
    button.addEventListener("click", () => { window.__bubbleClicks += 1; button.classList.add("active"); });
    window.__bubbleTip = createRailBubbleTooltip();
    window.__bubbleTip.wire(button);
  });
}

it("shows current bubble facts on mouse hover and keyboard focus", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBubble(page, basePath);
    const button = page.locator(".rail-bubble");
    await button.hover();
    const tooltip = page.locator(".rail-bubble-tooltip");
    await tooltip.waitFor();
    assert.equal(await tooltip.getAttribute("role"), "tooltip");
    assert.equal(await button.getAttribute("aria-describedby"), await tooltip.getAttribute("id"));
    assert.match(await tooltip.textContent(), /Rail helper.*Fix the conversation header.*Idle.*Claude Code · Claude Opus 5/s);
    await captureLayout(page, "rail-bubble-tooltip-hover-light.png");
    await page.mouse.move(0, 0);
    await tooltip.waitFor({ state: "detached" });
    await button.focus();
    await tooltip.waitFor();
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await captureLayout(page, "rail-bubble-tooltip-focus-dark.png");
    await page.keyboard.press("Escape");
    await tooltip.waitFor({ state: "detached" });
    assert.equal(await button.getAttribute("aria-describedby"), null);
  }, { width: 390, height: 760 });
}, 30_000);

it("clamps a long production tooltip inside every viewport edge", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBubble(page, basePath);
    const button = page.locator(".rail-bubble");
    await button.evaluate((node) => {
      node.dataset.railTooltip = JSON.stringify({
        name: "Long agent name ".repeat(12), work: "Long current work ".repeat(20),
        status: "idle", purpose: "Long purpose ".repeat(25), provider: "Claude Code", model: "Claude Opus 5",
      });
    });
    const tooltip = page.locator(".rail-bubble-tooltip");
    for (const edge of ["bottom", "top"]) {
      await button.evaluate((node, position) => {
        node.style.top = position === "top" ? "2px" : "auto";
        node.style.bottom = position === "bottom" ? "2px" : "auto";
      }, edge);
      await button.hover();
      await tooltip.waitFor();
      const box = await tooltip.boundingBox();
      assert.ok(box.x >= 7.5, `${edge}: tooltip crossed left edge`);
      assert.ok(box.y >= 7.5, `${edge}: tooltip crossed top edge`);
      assert.ok(box.x + box.width <= 232.5, `${edge}: tooltip crossed right edge`);
      assert.ok(box.y + box.height <= 152.5, `${edge}: tooltip crossed bottom edge`);
      await captureLayout(page, `rail-bubble-tooltip-${edge}-clamp.png`);
      await page.mouse.move(0, 0);
      await tooltip.waitFor({ state: "detached" });
    }
  }, { width: 240, height: 160 });
}, 30_000);

it("keeps a touch tap's normal bubble click without opening a hover tooltip", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBubble(page, basePath);
    await page.locator(".rail-bubble").tap();
    assert.equal(await page.evaluate(() => window.__bubbleClicks), 1);
    assert.equal(await page.locator(".rail-bubble.active").count(), 1);
    assert.equal(await page.locator(".rail-bubble-tooltip").count(), 0);
    assert.equal(await page.locator(".rail-bubble").getAttribute("aria-describedby"), null);
  }, { width: 390, height: 760, hasTouch: true });
}, 30_000);
