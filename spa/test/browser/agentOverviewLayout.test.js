import assert from "node:assert/strict";
import { it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { loadChatOverviewModules, seedChatOverview } from "./chatOverviewSeed.mjs";

const markup = `<div id="shell"><aside id="inbox-rail"></aside><div id="view">
  <header id="toolbar">Build / Workspace</header>
  <div id="view-body"><nav id="dir-rail"></nav>
    <main id="root"><h1>Workspace</h1><button id="workspace-action">Review changes</button></main>
    <aside id="agent-rail" aria-label="Agents"></aside>
  </div><div id="console-region"></div>
</div></div>`;

const rects = () => {
  const rect = (selector) => {
    const { x, y, width, height } = document.querySelector(selector).getBoundingClientRect();
    return { x, y, width, height };
  };
  const work = rect("#root");
  const point = { x: work.x + 35, y: work.y + 80 };
  return {
    work, rail: rect("#agent-rail"), strip: rect(".rail-strip"),
    panel: document.querySelector(".rail-panel") ? rect(".rail-panel") : null,
    workHit: document.elementFromPoint(point.x, point.y)?.closest("#root")?.id || null,
  };
};

const sameRect = (actual, expected) => {
  for (const key of ["x", "y", "width", "height"]) {
    assert.ok(Math.abs(actual[key] - expected[key]) <= 1,
      `${key}: ${actual[key]} differs from ${expected[key]}`);
  }
};

it("shows the agent overview in the chat panel's own box, docked and as a popover", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, {
      basePath,
      styles: "#toolbar{padding:12px 20px} #root{padding:24px} #root h1{font-size:20px}",
    });
    await loadBrowserModules(page, {
      rail: "src/core/agentRail.js",
      cache: "src/core/localCache.js",
    }, basePath);
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      await writeCached({ deviceId: "layout-device", entityId: "layout-run", kind: "row", sub: "" }, {
        run_id: "layout-run", project_id: "layout-project", agents: [{
          id: "layout-agent", ordinal: 1, provider: "claude_adk", state: "live",
          name: "Layout agent", topic: "Checking overview bounds", working: true, unread_count: 0,
        }],
      });
      localStorage.setItem("build.rail.expanded", "1");
      window.__workspaceClicks = 0;
      document.querySelector("#workspace-action").addEventListener("click", () => { window.__workspaceClicks += 1; });
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "project", deviceId: "layout-device", projectId: "layout-project",
        entityId: "layout-run", call: async (method) => method === "models.list"
          ? { default_provider: "claude_adk", providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }] }
          : { items: [] },
      });
    });

    await page.waitForSelector(".rail-panel:not([aria-hidden='true'])");
    await page.waitForSelector(".rail-overview-toggle");
    const dockedPanel = await page.evaluate(rects);
    assert.ok(dockedPanel.panel.width > 0);
    await page.locator(".rail-overview-toggle").click();
    await page.waitForSelector("#rail-panel .rail-overview-list");
    await page.waitForFunction(() => !document.querySelector(".rail-panel")?.getAnimations().length);
    const dockedOverview = await page.evaluate(rects);
    // #148: the overview is a body of the one panel, so it has the panel's box.
    sameRect(dockedOverview.panel, dockedPanel.panel);
    sameRect(dockedOverview.strip, dockedPanel.strip);
    assert.equal(dockedOverview.workHit, "root");
    assert.ok(dockedOverview.work.x + dockedOverview.work.width <= dockedOverview.panel.x,
      "the docked overview must start to the right of the workspace");
    await page.locator("#workspace-action").click();
    assert.equal(await page.evaluate(() => window.__workspaceClicks), 1);

    await page.locator("#rail-panel .pinbtn").click();
    await page.waitForSelector("#agent-rail.rail-popover:not([data-panel-transition])");
    await page.waitForFunction(() => !document.querySelector(".rail-panel")?.getAnimations().length);
    const popoverOverview = await page.evaluate(rects);
    assert.equal(popoverOverview.workHit, "root");
    assert.equal(Math.round(popoverOverview.rail.width), Math.round(popoverOverview.strip.width));
    // The popover's notch points at the overview's control, measured the way
    // it is for a bubble: the control's middle, along the panel's edge. Read
    // while the card is out and still: the press on the work below dismisses
    // it, and a panel sliding shut no longer stands where its notch was set.
    const notch = await page.evaluate(() => {
      const panel = document.querySelector("#rail-panel");
      const toggle = document.querySelector(".rail-overview-toggle").getBoundingClientRect();
      return { anchor: panel.dataset.anchor, offset: panel.style.getPropertyValue("--rail-anchor"),
        wanted: `${Math.round(toggle.top + toggle.height / 2 - panel.getBoundingClientRect().top)}px` };
    });
    assert.equal(notch.anchor, "overview");
    assert.equal(notch.offset, notch.wanted);
    await page.locator("#workspace-action").click();
    assert.equal(await page.evaluate(() => window.__workspaceClicks), 2);

    // Another bubble while it is out: the same card, now the conversation.
    await page.locator('[data-bubble="agent"]').click();
    await page.waitForFunction(() => !document.querySelector("#rail-panel .rail-overview-list"));
    await page.waitForFunction(() => !document.querySelector(".rail-panel")?.getAnimations().length);
    const popoverPanel = await page.evaluate(rects);
    sameRect(popoverOverview.panel, popoverPanel.panel);
    sameRect(popoverOverview.strip, popoverPanel.strip);
  }, { width: 1320, height: 850 });
}, 30_000);

it("opens the existing create-agent view for a workspace route", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath });
    await loadBrowserModules(page, {
      rail: "src/core/agentRail.js",
      cache: "src/core/localCache.js",
    }, basePath);
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      await writeCached({ deviceId: "layout-device", entityId: "workspace-run", kind: "row", sub: "" }, {
        entity_id: "workspace-run", project_id: "layout-project", agents: [{
          id: "layout-agent", ordinal: 1, provider: "codex", state: "live", name: "Existing agent",
        }],
      });
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "workspace", deviceId: "layout-device", projectId: "layout-project", workspaceId: "workspace-2",
        entityId: "workspace-run", projectAgent: { projectId: "layout-project" }, addingAgent: true,
        call: async (method) => method === "models.list"
          ? { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] }
          : { items: [] },
      });
    });
    await page.waitForSelector(".rail-panel:not([aria-hidden='true']) .rail-newagent");
  });
}, 30_000);

it("keeps overview scope on the workspace page after opening the project agent", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath });
    await loadBrowserModules(page, {
      rail: "src/core/agentRail.js",
      cache: "src/core/localCache.js",
      merge: "src/core/feedMerge.js",
    }, basePath);
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      const { stampWorkspace } = window.__layoutModules.merge;
      const deviceId = "layout-device";
      const projectId = "layout-project";
      const row = (entityId, agentId, name) => ({
        entity_id: entityId, project_id: projectId,
        agents: [{ id: agentId, name, ordinal: 1, provider: "codex", state: "live" }],
      });
      for (const [entityId, agentId, name] of [
        ["project-run", "project-agent", "Project agent"],
        ["workspace-run", "workspace-agent", "Current workspace agent"],
        ["other-run", "other-agent", "Other workspace agent"],
      ]) {
        await writeCached({ deviceId, entityId, kind: "row", sub: "" }, row(entityId, agentId, name));
      }
      await writeCached({ deviceId, entityId: "", kind: "workspaces" }, [
        { id: "workspace-1", project_id: projectId, entity_id: "workspace-run", name: "Current workspace" },
        { id: "workspace-2", project_id: projectId, entity_id: "other-run", name: "Other workspace" },
      ].map((workspace) => stampWorkspace(workspace, deviceId)));
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), {
        kind: "workspace", deviceId, projectId, workspaceId: "workspace-1", entityId: "workspace-run",
        projectAgent: { projectId, entityId: "project-run", name: "Build" },
        call: async (method) => method === "models.list"
          ? { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] }
          : { items: [] },
      });
    });

    await page.waitForSelector('[data-bubble="agent"][data-agent="workspace-agent"]', { timeout: 5000 });
    await page.waitForSelector('[data-bubble="project"][data-agent="project-run"]', { timeout: 5000 });
    await page.locator('[data-bubble="project"]').click();
    await page.waitForFunction(() => document.querySelector("#rail-panel .rail-who")?.title === "Project agent", null, { timeout: 5000 });
    await page.waitForSelector('[data-bubble="agent"][data-agent="workspace-agent"]', { timeout: 5000 });
    await page.locator(".rail-overview-toggle").click();
    await page.waitForSelector('[data-overview-agent="workspace-agent"]', { timeout: 5000 });
    const agents = await page.locator(".rail-overview-row").evaluateAll((rows) => rows.map((row) => row.dataset.overviewAgent));
    assert.deepEqual(agents.sort(), ["project-agent", "workspace-agent"]);
  });
}, 30_000);

// #117, unmocked end to end: the production rail on a cached project, moved
// from the workspace's overview out to the project's, capped there, and back
// into another workspace through its heading and its See all.
it("moves the chat overview between one workspace and the whole project", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath });
    await loadChatOverviewModules(page, basePath);
    await page.evaluate(seedChatOverview);
    // Every section; an unwatched agent is under its workspace wearing the
    // not-watching mark (#105, #186).
    const sections = () => page.locator(".rail-overview-list > .rail-overview-section").evaluateAll((all) => all.map((section) => ({
      name: section.getAttribute("aria-label"),
      agents: [...section.querySelectorAll(".rail-overview-row")].map((row) => row.dataset.overviewAgent),
      seeAll: !!section.querySelector(".rail-overview-see-all"),
    })));
    const sectionsNamed = (names) => page.waitForFunction((wanted) => JSON.stringify([...document
      .querySelectorAll(".rail-overview-list > .rail-overview-section")].map((section) => section.getAttribute("aria-label")))
      === JSON.stringify(wanted), names, { timeout: 5000 });
    const notWatching = () => page.locator('.rail-overview-section .rail-overview-row-unwatched')
      .evaluateAll((rows) => rows.map((row) => row.dataset.overviewAgent));

    await page.waitForSelector('.rail-bubble-add + .rail-overview-toggle', { timeout: 5000 });
    await page.locator(".rail-overview-toggle").click();
    await sectionsNamed(["Project agents", "chat-overview-nav"]);
    await page.locator("#rail-panel .rail-overview-up").click();
    // #186: what needs the reader first (spa-flaky-tests has an unread), then
    // what is working (chat-overview-nav), then the quiet, then the empty.
    await sectionsNamed(["Project agents", "spa-flaky-tests", "chat-overview-nav", "landing-page",
      "relay-candidates", "review-system-plan"]);
    assert.deepEqual((await sections()).map(({ agents, seeAll }) => [agents.length, seeAll]),
      [[1, false], [3, true], [1, false], [2, false], [0, false], [0, false]]);
    assert.deepEqual(await notWatching(), ["quiet-review"]);
    // A workspace with no agents keeps its way in and its +.
    assert.equal(await page.locator('.rail-overview-section[aria-label="review-system-plan"] .rail-overview-add').count(), 1);
    await page.locator('.rail-overview-open[data-overview-scope="workspace-new"]').click();
    await sectionsNamed(["Project agents", "review-system-plan"]);
    await page.locator("#rail-panel .rail-overview-up").click();
    assert.equal(await page.locator("#rail-panel .rail-overview-up").count(), 0);

    await page.locator('.rail-overview-list > .rail-overview-section .rail-overview-open[data-overview-scope="workspace-quiet"]').click();
    await sectionsNamed(["Project agents", "landing-page"]);
    assert.deepEqual(await notWatching(), ["quiet-review"]);
    await page.locator("#rail-panel .rail-overview-up").click();
    await page.locator('.rail-overview-see-all[data-overview-scope="workspace-busy"]').click();
    await sectionsNamed(["Project agents", "spa-flaky-tests"]);
    // Unread and working agents lead; the rest by last word.
    assert.deepEqual((await sections())[1].agents, ["busy-2", "busy-3", "busy-5", "busy-4", "busy-1"]);
  });
}, 30_000);

// #192: the right of every workspace heading is three fixed slots — unread
// pill, working dot, + — on one gap, so the dot and the + stand in the same
// columns on every heading, the dot is drawn (idle) even when nothing works,
// and the +'s 44px press does not widen the visual gap.
it("keeps the heading's pill, dot and + on one gap and in one column", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath });
    await loadChatOverviewModules(page, basePath, { tracker: "src/core/trackerCache.js", fixture: "test/agentsOverviewFixture.js" });
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      const { stampWorkspace } = window.__layoutModules.merge;
      const { writeTasksRecord } = window.__layoutModules.tracker;
      const { writeAgentsOverviewFixture, overviewRailContext } = window.__layoutModules.fixture;
      await writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeTasksRecord });
      localStorage.setItem("build.rail.expanded", "1");
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), overviewRailContext());
    });
    await page.waitForSelector(".rail-overview-toggle", { timeout: 5000 });
    await page.locator(".rail-overview-toggle").click();
    await page.waitForFunction(() => document.querySelectorAll("#rail-panel .rail-overview-section").length >= 5, null, { timeout: 5000 });
    await page.waitForFunction(() => !document.querySelector(".rail-panel")?.getAnimations().length);

    const heads = await page.evaluate(() => {
      const box = (node) => { const { left, right, top, bottom, width, height } = node.getBoundingClientRect(); return { left, right, top, bottom, width, height }; };
      const glyph = (button) => { const range = document.createRange(); range.selectNodeContents(button); return box(range); };
      const list = document.querySelector(".rail-overview-list");
      return {
        scrollsSideways: list.scrollWidth > list.clientWidth,
        heads: [...document.querySelectorAll('.rail-overview-section[aria-label]:not([aria-label="Project agents"]) .rail-overview-section-head')].map((head) => {
          const pill = head.querySelector(".rail-overview-need:not(.is-error)");
          const dot = head.querySelector(".rail-overview-live, .rail-overview-idle");
          const add = head.querySelector(".rail-overview-add");
          return { name: head.closest("section").getAttribute("aria-label"), head: box(head),
            pill: pill && box(pill), dot: dot && { ...box(dot), className: dot.className }, add: box(add), plus: glyph(add) };
        }),
      };
    });
    assert.equal(heads.heads.length, 4, "every workspace has a heading");
    assert.equal(heads.scrollsSideways, false, "the +'s press target stays inside the list");
    const first = heads.heads[0];
    for (const head of heads.heads) {
      assert.ok(head.dot, `${head.name}: the working dot is drawn`);
      assert.ok(Math.abs(head.head.height - 36) <= 1, `${head.name}: heading height ${head.head.height}`);
      assert.ok(Math.abs(head.add.width - 44) <= 1 && Math.abs(head.add.height - 44) <= 1, `${head.name}: + press ${head.add.width}x${head.add.height}`);
      // One column for the dot and one for the +, whatever the heading holds.
      assert.ok(Math.abs(head.dot.left - first.dot.left) <= 1, `${head.name}: dot at ${head.dot.left}, first at ${first.dot.left}`);
      assert.ok(Math.abs(head.plus.left - first.plus.left) <= 1, `${head.name}: + at ${head.plus.left}, first at ${first.plus.left}`);
      const dotToPlus = head.plus.left - head.dot.right;
      if (head.pill) {
        const pillToDot = head.dot.left - head.pill.right;
        assert.ok(Math.abs(pillToDot - dotToPlus) <= 1, `${head.name}: pill→dot ${pillToDot} vs dot→+ ${dotToPlus}`);
      }
      assert.ok(dotToPlus >= 6 && dotToPlus <= 10, `${head.name}: dot→+ ${dotToPlus} is not the rows' gap`);
    }
    // The project's heading has no +, and its dot stands in the same column.
    const projectDot = await page.evaluate(() => document.querySelector('.rail-overview-section[aria-label="Project agents"] .rail-overview-idle')?.getBoundingClientRect().left);
    assert.ok(Math.abs(projectDot - first.dot.left) <= 1, `project dot at ${projectDot}, workspace dots at ${first.dot.left}`);
    const byName = Object.fromEntries(heads.heads.map((head) => [head.name, head]));
    assert.equal(byName["skrift-fixes"].dot.className, "rail-overview-live");
    assert.equal(byName["skrift-review"].dot.className, "rail-overview-idle");
    assert.ok(byName["skrift-review"].pill, "skrift-review carries its unread pill");
    assert.equal(byName["issue-implementation-audit"].pill, null, "no pill for nothing unread");
  }, { width: 1320, height: 850 });
}, 30_000);

// #192 review: the +'s 44px press reaches under the working dot, so a press
// on the dot has to open Add too, while the dot keeps its own hover — its
// status word is the tooltip, not the +'s — and the pill beside it stays inert.
it("opens Add from a press on the working dot as from the + itself, and the dot keeps its tooltip", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountLayout(page, markup, { basePath });
    await loadChatOverviewModules(page, basePath, { tracker: "src/core/trackerCache.js", fixture: "test/agentsOverviewFixture.js" });
    await page.evaluate(async () => {
      const { mountAgentRail } = window.__layoutModules.rail;
      const { writeCached } = window.__layoutModules.cache;
      const { stampWorkspace } = window.__layoutModules.merge;
      const { writeTasksRecord } = window.__layoutModules.tracker;
      const { writeAgentsOverviewFixture, overviewRailContext } = window.__layoutModules.fixture;
      await writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeTasksRecord });
      localStorage.setItem("build.rail.expanded", "1");
      window.__layoutRail = mountAgentRail(document.querySelector("#agent-rail"), overviewRailContext());
    });
    await page.waitForSelector(".rail-overview-toggle", { timeout: 5000 });
    await page.locator(".rail-overview-toggle").click();
    await page.waitForFunction(() => document.querySelectorAll("#rail-panel .rail-overview-section").length >= 5, null, { timeout: 5000 });
    await page.waitForFunction(() => !document.querySelector(".rail-panel")?.getAnimations().length);

    const centre = (name, selector) => page.evaluate(([sectionName, wanted]) => {
      const { left, right, top, bottom } = document.querySelector(`.rail-overview-section[aria-label="${sectionName}"] .rail-overview-section-head ${wanted}`).getBoundingClientRect();
      return { x: (left + right) / 2, y: (top + bottom) / 2, left, right, top, bottom };
    }, [name, selector]);
    const hashAfterClick = async (point) => {
      await page.evaluate(() => window.history.replaceState({}, "", window.location.pathname));
      await page.mouse.click(point.x, point.y);
      await page.waitForTimeout(50);
      return page.evaluate(() => window.location.hash);
    };
    const opensAdd = (hash, workspaceId) => hash.includes(`/workspace/${workspaceId}/`) && hash.includes("newAgent");

    // The tooltip a hover shows is the innermost hovered element's title.
    const hoveredTitle = async (point) => {
      await page.mouse.move(point.x, point.y);
      return page.evaluate(() => { const hovered = [...document.querySelectorAll(":hover")]; const top = hovered.at(-1);
        return { className: top?.className, title: top?.closest("[title]")?.title || null }; });
    };
    for (const [name, workspaceId, dotClass, word] of [["skrift-review", "ws-review", ".rail-overview-idle", "Nothing working"], ["skrift-fixes", "ws-fixes", ".rail-overview-live", "1 working"]]) {
      const dot = await centre(name, dotClass);
      assert.deepEqual(await hoveredTitle(dot), { className: dotClass.slice(1), title: word }, `${name}: hovering the dot shows its status`);
      assert.ok(opensAdd(await hashAfterClick(dot), workspaceId), `${name}: a press on the dot opens Add`);
      // Inside the +'s box but outside the heading's 36px band: still the +.
      const press = await centre(name, ".rail-overview-add");
      assert.ok(opensAdd(await hashAfterClick({ x: press.right - 3, y: press.top + 3 }), workspaceId), `${name}: a press at the +'s corner opens Add`);
      assert.ok(opensAdd(await hashAfterClick({ x: press.x, y: press.y }), workspaceId), `${name}: a press on the + opens Add`);
    }
    // The pill is not the +.
    const pill = await centre("skrift-review", ".rail-overview-need");
    assert.equal(await hashAfterClick(pill), "", "a press on the unread pill opens nothing");
    // The project's dot has no + under it: its own tooltip, and no Add.
    const projectDot = await centre("Project agents", ".rail-overview-idle");
    assert.deepEqual(await hoveredTitle(projectDot), { className: "rail-overview-idle", title: "Nothing working" });
    assert.equal(await hashAfterClick(projectDot), "", "a press on the project's dot opens nothing");
  }, { width: 1320, height: 850 });
}, 30_000);
