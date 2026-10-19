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

async function mountRail(page, basePath, { agent = {}, projectPin = false } = {}) {
  await mountLayout(page, markup(page.viewportSize().width > 600), { basePath });
  await loadBrowserModules(page, { rail: "src/core/agentRail.js", cache: "src/core/localCache.js" }, basePath);
  await page.evaluate(async ({ agent, projectPin }) => {
    const { mountAgentRail } = window.__layoutModules.rail;
    const { writeCached } = window.__layoutModules.cache;
    window.__railRow = {
      kind: "project", entity_id: "layout-run", run_id: "layout-run", project_id: "layout-project", agents: [{
        id: "layout-agent", ordinal: 1, provider: "claude_adk", state: "live",
        name: "Rail helper", topic: "Old work", model: "claude-opus-5", watched: true,
        working: false, unread_count: 0, ...agent,
      }],
    };
    window.__writeRailRow = (row) => writeCached({ deviceId: "layout-device", entityId: "layout-run", kind: "row", sub: "" }, row);
    await window.__writeRailRow(window.__railRow);
    if (projectPin) {
      await writeCached({ deviceId: "layout-device", entityId: "", kind: "project-agents-support" }, { projectWorkers: true });
      await writeCached({ deviceId: "layout-device", entityId: "layout-run", kind: "project-agents" }, {
        entity_id: "layout-run", pinned_agent_id: "layout-agent", agents: window.__railRow.agents,
      });
    }
    window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId: "layout-device", projectId: "layout-project", entityId: "layout-run",
      openAgentId: "layout-agent", panelOpen: true,
      call: async (method) => method === "models.list"
        ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
        : { items: [] },
    });
  }, { agent, projectPin });
  await page.waitForSelector(".rail-head");
}

it("retains the header controls while cached model changes update the bubble tooltip", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountRail(page, basePath);
    await page.evaluate(async () => {
      window.__originalRailHead = document.querySelector(".rail-head");
      await window.__writeRailRow({
        ...window.__railRow,
        agents: [{ ...window.__railRow.agents[0], active_model: "claude-sonnet-4-6" }],
      });
    });
    await page.waitForFunction(() => {
      const bubble = document.querySelector('[data-bubble="agent"][data-agent="layout-agent"]');
      return JSON.parse(bubble?.dataset.railTooltip || "{}").model?.includes("Sonnet");
    });
    assert.equal(await page.evaluate(() => document.querySelector(".rail-head") === window.__originalRailHead), true,
      "model updates must keep the standing header controls");
    assert.equal(await page.locator(".rail-head .agent-model-badge").count(), 0);
  });
}, 30_000);

for (const width of [1180, 390]) it(`keeps the ${width}px header and controls still through every erased and typed frame`, async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountRail(page, basePath);
    await page.waitForSelector(".rail-who-work");
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
        watch: rect(head.querySelector(".rail-watch")),
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
      for (const key of ["name", "harness", "watch", "head"]) {
        for (const dimension of ["x", "y", "width", "height"]) {
          assert.ok(Math.abs(frame[key][dimension] - samples[0][key][dimension]) <= 1, `${key}.${dimension} shifted`);
        }
      }
      assert.ok(frame.work.height >= 14, "work line collapsed while empty");
    }
    await captureLayout(page, `rail-two-line-header-${width}.png`);
    await page.evaluate(async () => {
      const head = document.querySelector(".rail-head");
      const work = head.querySelector(".rail-who-work");
      const box = (node) => {
        const rect = node.getBoundingClientRect();
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      };
      const sample = () => ({ text: work.textContent, work: box(work), name: box(head.querySelector(".rail-who-name")),
        harness: box(head.querySelector(".rail-harness-icon")),
        watch: box(head.querySelector(".rail-watch")), head: box(head) });
      window.__clearSamples = [sample()];
      window.__clearObserver = new MutationObserver(() => { if (work.isConnected) window.__clearSamples.push(sample()); });
      window.__clearObserver.observe(work, { childList: true, characterData: true, subtree: true });
      await window.__writeRailRow({ ...window.__railRow, agents: [{ ...window.__railRow.agents[0], topic: "" }] });
    });
    await page.waitForFunction(() => window.__clearSamples?.some((frame) => frame.text && frame.text !== "Updated work"),
      null, { timeout: 2000 });
    await captureLayout(page, `rail-clearing-header-${width}.png`);
    await page.waitForFunction(() => {
      const work = document.querySelector(".rail-who-work");
      return work?.textContent === "Starting" && !work.dataset.titleMotion;
    });
    const cleared = await page.evaluate(() => { window.__clearObserver.disconnect(); return window.__clearSamples; });
    assert.ok(cleared.some((frame) => frame.text.length < "Updated work".length && frame.text.length > 0),
      "clearing the topic erased visible characters before Starting");
    assert.ok(cleared.some((frame) => frame.text === ""), "the fully erased work frame was measured");
    for (const frame of cleared) {
      assert.ok(frame.work.height >= 14, "work line kept its space throughout clearing");
      for (const key of ["name", "harness", "watch", "head"]) {
        for (const dimension of ["x", "y", "width", "height"]) {
          assert.ok(Math.abs(frame[key][dimension] - cleared[0][key][dimension]) <= 1,
            `${key}.${dimension} shifted while clearing`);
        }
      }
    }
    assert.equal(await page.locator(".rail-who-name").textContent(), "Rail helper");
    assert.equal(await page.locator(".rail-who").count(), 1);
    assert.equal(await page.locator(".rail-who").evaluate((who) => who.children.length), 2);
    assert.equal(await page.locator(".rail-who-work").evaluate((work) => work.classList.contains("rail-who-starting")), true);
    await captureLayout(page, `rail-cleared-starting-header-${width}.png`);
  }, { width, height: 840 });
}, 30_000);

for (const state of ["starting", "live"]) it(`shimmers Starting on the work line of a topicless ${state} agent`, async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountRail(page, basePath, { agent: { state, topic: "" } });
    const work = page.locator(".rail-who-work");
    await work.waitFor({ timeout: 5000 });
    assert.equal(await work.textContent(), "Starting");
    assert.equal(await work.evaluate((node) => node.classList.contains("rail-who-starting")), true);
    assert.equal(await work.evaluate((node) => getComputedStyle(node).animationName !== "none"), true);
    assert.equal(await page.locator(".rail-who-name").textContent(), "Rail helper");
    assert.equal(await page.locator(".rail-who-name").evaluate((node) => node.classList.contains("rail-who-starting")), false);
    await captureLayout(page, `rail-starting-header-${state}-390.png`);
    await page.evaluate(async () => {
      await window.__writeRailRow({ ...window.__railRow, agents: [{ ...window.__railRow.agents[0], state: "live", topic: "Inspect project layout" }] });
    });
    await page.waitForFunction(() => {
      const work = document.querySelector(".rail-who-work");
      return work?.textContent === "Inspect project layout" && !work.dataset.titleMotion;
    });
    assert.equal(await work.evaluate((node) => node.classList.contains("rail-who-starting")), false);
    await captureLayout(page, `rail-starting-topic-arrived-${state}-390.png`);
  }, { width: 390, height: 840 });
}, 30_000);

it("names an unnamed project pin Project agent in its header and tooltip", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountRail(page, basePath, { projectPin: true, agent: {
      name: null, project_role: "coordinator", topic: "Coordinate the project",
      provider: "pi", model: "gemini-2.5-pro",
    } });
    await page.waitForSelector('[data-bubble="agent"][data-agent="layout-agent"]');
    assert.equal(await page.locator(".rail-who-name").textContent(), "Project agent");
    const button = page.locator('[data-bubble="agent"][data-agent="layout-agent"]');
    assert.equal(await button.getAttribute("aria-label"), "Project agent");
    const facts = JSON.parse(await button.getAttribute("data-rail-tooltip"));
    assert.equal(facts.name, "Project agent");
    assert.equal(facts.purpose, "Project agent");
    assert.match(facts.model, /gemini-2\.5-pro/i, "the pin retains an unfamiliar model in its tooltip");
    await button.hover();
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Project agent");
    await captureLayout(page, "rail-project-agent-pin-390.png");
  }, { width: 390, height: 840 });
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

async function mountTwoBubbles(page, basePath) {
  await mountBubble(page, basePath);
  page.setDefaultTimeout(5000);
  await page.evaluate(() => {
    const { bubbleHtml } = window.__layoutModules.rail;
    const host = document.querySelector("#bubble-fixture");
    host.insertAdjacentHTML("beforeend", bubbleHtml({
      type: "agent", id: "second-agent", title: "Second helper", label: "S", unread: 0,
      tooltip: { name: "Second helper", work: "Review the rail", status: "idle" },
    }));
    const second = host.lastElementChild;
    second.style.position = "fixed";
    second.style.right = "44px";
    second.style.bottom = "2px";
    window.__bubbleTip.wire(second);
  });
}

it("shows on hover and Tab focus, then hides after mouse click and pointer leave", async () => {
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
    await button.click();
    assert.equal(await page.evaluate(() => window.__bubbleClicks), 1);
    await page.mouse.move(0, 0);
    await tooltip.waitFor({ state: "detached", timeout: 2000 });
    assert.equal(await button.evaluate((node) => document.activeElement === node), true,
      "mouse click leaves the button focused without retaining a hover tooltip");
    await button.evaluate((node) => node.blur());
    await page.keyboard.press("Tab");
    assert.equal(await button.evaluate((node) => node.matches(":focus-visible")), true);
    await tooltip.waitFor();
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await captureLayout(page, "rail-bubble-tooltip-focus-dark.png");
    await page.keyboard.press("Escape");
    await tooltip.waitFor({ state: "detached" });
    assert.equal(await button.getAttribute("aria-describedby"), null);
  }, { width: 390, height: 760 });
}, 30_000);

it("keeps A's hovered tooltip when A blurs while the pointer stays on A", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountTwoBubbles(page, basePath);
    const first = page.locator('[data-agent="layout-agent"]');
    const tooltip = page.locator(".rail-bubble-tooltip");
    await first.hover();
    await first.focus();
    await tooltip.waitFor();
    await first.evaluate((button) => button.blur());
    assert.equal(await tooltip.count(), 1);
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Rail helper");
    assert.equal(await first.getAttribute("aria-describedby"), await tooltip.getAttribute("id"));
    await page.mouse.move(0, 0);
    await tooltip.waitFor({ state: "detached" });
  }, { width: 390, height: 760 });
}, 30_000);

it("leaving A after Tab focuses B keeps B's tooltip and description", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountTwoBubbles(page, basePath);
    const first = page.locator('[data-agent="layout-agent"]');
    const second = page.locator('[data-agent="second-agent"]');
    const tooltip = page.locator(".rail-bubble-tooltip");
    await first.hover();
    await first.focus();
    await page.keyboard.press("Tab");
    assert.equal(await second.evaluate((button) => button.matches(":focus-visible")), true);
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Second helper");
    const secondTipId = await second.getAttribute("aria-describedby");
    assert.equal(secondTipId, await tooltip.getAttribute("id"));
    await page.mouse.move(0, 0);
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Second helper");
    assert.equal(await second.getAttribute("aria-describedby"), secondTipId);
    assert.equal(await first.getAttribute("aria-describedby"), null);
    await captureLayout(page, "rail-bubble-tooltip-second-focus-390.png");
    await second.evaluate((button) => {
      button.dataset.railTooltip = JSON.stringify({ name: "Updated second helper", work: "Review the rail", status: "idle" });
      button.removeAttribute("aria-describedby");
    });
    await page.evaluate(() => window.__bubbleTip.refresh());
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Updated second helper");
    assert.equal(await second.getAttribute("aria-describedby"), secondTipId);
    assert.equal(await first.getAttribute("aria-describedby"), null);
    await first.hover();
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Rail helper");
    await page.mouse.move(0, 0);
    assert.equal(await page.locator(".rail-bubble-tooltip-name").textContent(), "Updated second helper");
    assert.equal(await second.getAttribute("aria-describedby"), secondTipId);
    assert.equal(await first.getAttribute("aria-describedby"), null);
    await captureLayout(page, "rail-bubble-tooltip-focus-restored-390.png");
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

async function measureShortTooltip(page) {
  return page.evaluate(() => {
      const tip = document.querySelector(".rail-bubble-tooltip");
      const work = tip.querySelector(".rail-bubble-tooltip-work");
      const status = tip.querySelector(".rail-bubble-tooltip-status");
      const purpose = [...tip.querySelectorAll(".rail-bubble-tooltip-detail")]
        .find((line) => line.textContent === "Project agent");
      const watch = [...tip.querySelectorAll(".rail-bubble-tooltip-detail")]
        .find((line) => line.textContent === "Not watching");
      const descendants = [...tip.querySelectorAll(".rail-bubble-tooltip-detail")]
        .find((line) => line.textContent === "2 agents working");
      const detail = [...tip.querySelectorAll(".rail-bubble-tooltip-detail")]
        .find((line) => line.textContent.includes("Claude Code"));
      const news = tip.querySelector(".rail-bubble-tooltip-news");
      const tipBox = tip.getBoundingClientRect();
      const withinTip = (node) => {
        const box = node.getBoundingClientRect();
        return box.top >= tipBox.top && box.bottom <= tipBox.bottom && box.left >= tipBox.left && box.right <= tipBox.right;
      };
      const workStyle = getComputedStyle(work);
      const tipStyle = getComputedStyle(tip);
      return {
        tip: { left: tipBox.left, top: tipBox.top, right: tipBox.right, bottom: tipBox.bottom },
        clientHeight: tip.clientHeight, scrollHeight: tip.scrollHeight,
        pointerEvents: tipStyle.pointerEvents, overflowY: tipStyle.overflowY,
        name: tip.querySelector(".rail-bubble-tooltip-name").textContent,
        nameVisible: withinTip(tip.querySelector(".rail-bubble-tooltip-name")),
        purposeVisible: purpose ? withinTip(purpose) : false,
        watchVisible: watch ? withinTip(watch) : false,
        descendantsVisible: descendants ? withinTip(descendants) : false,
        newsVisible: news ? withinTip(news) : false,
        statusVisible: withinTip(status), detailVisible: detail ? withinTip(detail) : false,
        status: status.textContent, detail: detail?.textContent || "", news: news?.textContent || "",
        workLines: work.getBoundingClientRect().height / Number.parseFloat(workStyle.lineHeight),
        workClamp: Number.parseInt(workStyle.webkitLineClamp, 10),
        workClipped: work.scrollHeight > work.clientHeight,
      };
  });
}

function assertShortTooltip(measured, { width, height }) {
    assert.ok(measured.tip.left >= 7.5 && measured.tip.right <= width - 7.5,
      `horizontal tooltip bounds ${JSON.stringify(measured.tip)} exceed ${width}px viewport`);
    assert.ok(measured.tip.top >= 7.5 && measured.tip.bottom <= height - 7.5,
      `vertical tooltip bounds ${JSON.stringify(measured.tip)} exceed ${height}px viewport`);
    assert.equal(measured.scrollHeight <= measured.clientHeight, true, "tooltip content fits without scrolling");
    assert.equal(measured.pointerEvents, "none");
    assert.ok(!["auto", "scroll"].includes(measured.overflowY), "tooltip is not a scroll surface");
    assert.equal(measured.nameVisible, true);
    assert.equal(measured.purposeVisible, true);
    assert.equal(measured.watchVisible, true);
    assert.equal(measured.descendantsVisible, true);
    assert.equal(measured.newsVisible, true);
    assert.equal(measured.statusVisible, true);
    assert.equal(measured.detailVisible, true);
    assert.match(measured.status, /^Working · \d/);
    assert.equal(measured.detail, "Claude Code · Claude Opus 5");
    assert.equal(measured.news, "1 unread");
    assert.ok(measured.workLines <= 4.1, "long work uses at most four lines");
    assert.ok(measured.workClamp >= 1 && measured.workClamp <= 4, "long work has an ellipsis clamp");
    assert.equal(measured.workClipped, true, "long work is truncated inside its own line box");
}

it("keeps fixed tooltip facts visible through short viewport resize and cached content refresh", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBubble(page, basePath);
    const button = page.locator(".rail-bubble");
    await button.evaluate((node) => {
      node.style.right = "auto";
      node.style.bottom = "auto";
      node.style.left = "2px";
      node.style.top = "2px";
      node.dataset.railTooltip = JSON.stringify({
        name: "Rail helper", work: "Describe every step of the current work in detail. ".repeat(50),
        purpose: "Project agent", status: "working", watch: "Not watching",
        descendants: "2 agents working", news: "1 unread",
        workingTime: { since: new Date(Date.now() - 83_000).toISOString() },
        provider: "Claude Code", model: "Claude Opus 5",
      });
    });
    await button.hover();
    const tooltip = page.locator(".rail-bubble-tooltip");
    await tooltip.waitFor();
    const tipId = await tooltip.getAttribute("id");
    assertShortTooltip(await measureShortTooltip(page), { width: 390, height: 260 });
    await captureLayout(page, "rail-bubble-tooltip-short-viewport-390x260.png");
    await page.setViewportSize({ width: 320, height: 220 });
    await page.waitForFunction(() => {
      const tip = document.querySelector(".rail-bubble-tooltip");
      if (!tip) return false;
      const box = tip.getBoundingClientRect();
      return box.left >= 7.5 && box.right <= 312.5 && box.top >= 7.5 && box.bottom <= 212.5;
    }, null, { timeout: 5000 });
    assertShortTooltip(await measureShortTooltip(page), { width: 320, height: 220 });
    await captureLayout(page, "rail-bubble-tooltip-resized-320x220.png");
    await button.evaluate((node) => {
      const facts = JSON.parse(node.dataset.railTooltip);
      node.dataset.railTooltip = JSON.stringify({ ...facts, name: "Updated rail helper",
        work: "A newer cached work description. ".repeat(50) });
      node.removeAttribute("aria-describedby");
    });
    await page.evaluate(() => window.__bubbleTip.refresh());
    const refreshed = await measureShortTooltip(page);
    assert.equal(refreshed.name, "Updated rail helper");
    assertShortTooltip(refreshed, { width: 320, height: 220 });
    assert.equal(await button.getAttribute("aria-describedby"), tipId);
    await captureLayout(page, "rail-bubble-tooltip-refreshed-320x220.png");
    await page.setViewportSize({ width: 320, height: 180 });
    await page.waitForFunction(() => {
      const tip = document.querySelector(".rail-bubble-tooltip");
      if (!tip) return false;
      const box = tip.getBoundingClientRect();
      return box.left >= 7.5 && box.right <= 312.5 && box.top >= 7.5 && box.bottom <= 172.5;
    }, null, { timeout: 5000 });
    const compact = await measureShortTooltip(page);
    await captureLayout(page, "rail-bubble-tooltip-all-facts-320x180.png");
    assert.equal(compact.name, "Updated rail helper");
    assertShortTooltip(compact, { width: 320, height: 180 });
    assert.equal(await button.getAttribute("aria-describedby"), tipId);
  }, { width: 390, height: 260 });
}, 30_000);

it("keeps a touch tap's normal bubble click without opening a hover tooltip", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountBubble(page, basePath);
    await page.locator(".rail-bubble").tap();
    assert.equal(await page.evaluate(() => window.__bubbleClicks), 1);
    assert.equal(await page.locator(".rail-bubble.active").count(), 1);
    assert.equal(await page.locator(".rail-bubble-tooltip").count(), 0);
    assert.equal(await page.locator(".rail-bubble").getAttribute("aria-describedby"), null);
    await captureLayout(page, "rail-bubble-touch-tap-390.png");
  }, { width: 390, height: 760, hasTouch: true });
}, 30_000);
