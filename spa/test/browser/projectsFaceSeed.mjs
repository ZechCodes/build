import { readFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

const html = await readFile(new URL("../../index.html", import.meta.url), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];

// Cached fixtures and an in-memory device session; no bridge or account.
export async function mountProjectsFace(page, basePath, { standing = false, emptyLive = false } = {}) {
  await mountLayout(page, body, { basePath });
  await loadBrowserModules(page, {
    app: "src/app.js", cache: "src/core/localCache.js", feed: "src/core/taskFeed.js",
    inbox: "src/core/inboxShell.js", view: "src/core/inboxView.js", contexts: "src/core/deviceContexts.js",
    tracker: "src/core/trackerCache.js",
  }, basePath);
  await page.evaluate(async ({ standing, emptyLive }) => {
    const { app, cache, feed, inbox, view, contexts, tracker } = window.__layoutModules;
    const deviceId = "projects-review";
    app.App.devices = [{ id: deviceId, name: "Workshop", status: "online" }];
    app.App.selectedDeviceId = deviceId;
    app.App.route = standing
      ? { name: "trackerTask", deviceId, projectId: "build", taskId: "land-tracker" }
      : { name: "project", deviceId, projectId: "build" };
    const now = Date.now();
    const hour = 3_600_000;
    const project = (id, name, age) => ({ id, name, deviceId, projectKey: `${deviceId}/${id}`,
      session_started_ms: now - age, last_activity_ms: now - age });
    const projects = [project("zulu", "Zulu", hour * 3), project("build", "Build", hour),
      project("alpha", "Alpha", hour * 24 * 30), project("bravo", "Bravo", 0)];
    const workspace = (id, name, projectId, age) => ({ id, name, project_id: projectId, deviceId,
      projectKey: `${deviceId}/${projectId}`, workspaceKey: `${deviceId}/${id}`,
      status: "ready", managed: true, entity_id: id, directories: [{ id: "src", is_git: true }],
      session_started_ms: now - age, last_activity_ms: now - age });
    const workspaces = [workspace("zulu-live", "Running work", "zulu", hour * 3),
      workspace("build-live", "Current work", "build", emptyLive ? hour * 48 : hour),
      workspace("build-quiet", "Older workspace", "build", hour * 48),
      workspace("alpha-quiet", "Last month's work", "alpha", hour * 24 * 30)];
    const items = workspaces.map((workspace) => ({ kind: "branch", run_id: workspace.entity_id,
      project_id: workspace.project_id, deviceId, projectKey: workspace.projectKey,
      agents: [{ id: `${workspace.id}-agent`, watched: true, working: workspace.id === "zulu-live",
        unread_count: workspace.id === "build-live" && !emptyLive ? 2 : 0 }] }));
    if (emptyLive) {
      // The project agent is now an ordinary row too. Age its conversation
      // alongside its workspaces to keep this an actually empty live section.
      projects.find((project) => project.id === "build").entity_id = "build-project-run";
      items.push({ kind: "branch", run_id: "build-project-run", project_id: "build", deviceId,
        projectKey: `${deviceId}/build`, session_started_ms: now - hour * 48,
        last_activity_ms: now - hour * 48, agents: [] });
    }
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, workspaces);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { projects, workspaces, items, runs: items });
    // Two watched tasks gone quiet under Build (#516): one with a reason line.
    const quietAt = new Date(now - hour * 72).toISOString();
    const task = (id, number, title, over = {}) => ({ id, number, title, project_id: "build", state: "open",
      status: "in_progress", watched: true, assignee: null, unread_count: 0, created_at: quietAt,
      updated_at: quietAt, ...over });
    const tasks = [task("land-tracker", 25, "Land the issue tracker in the rail"),
      task("release-check", 348, "Release checklist for the public launch", { assignee: { kind: "user" } })];
    await tracker.writeTasksRecord(deviceId, "build", tracker.tasksRecord(tasks, []));
    for (const one of tasks) await tracker.writeTaskRecord(deviceId, "build", one.id, { task: one, timeline: [] });
    contexts.adoptDeviceSession({ deviceId, call: async () => ({}), close() {}, peer() {}, onCarrier() {}, onPush() {} });
    await inbox.initInboxRail();
    await feed.startFeed();
    view.setInboxView("projects");
    if (innerWidth <= 900) document.body.classList.add("inbox-collapsed", "inbox-popover-open");
  }, { standing, emptyLive });
  await page.locator('.inbox-project[data-project="projects-review/build"]').waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(0, 0);
}

export async function inspectProjectsFace(page) {
  return page.locator(".inbox-project").evaluateAll((blocks) => blocks.map((block) => {
    const style = getComputedStyle(block);
    const probe = block.appendChild(document.createElement("span"));
    probe.style.background = "var(--accent-soft)";
    const accentSoft = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return { name: block.querySelector(".inbox-project-name").textContent.trim(),
      active: block.classList.contains("active"), background: style.backgroundColor,
      accentSoft,
      border: style.borderWidth, shadow: style.boxShadow };
  }));
}

/** Open one block's Recent and wait for its quiet rows. */
export async function openRecent(page, projectKey) {
  await page.locator(`[data-recent-toggle="${projectKey}"]`).click();
  await page.locator(`.inbox-project[data-project="${projectKey}"] .inbox-recent .inbox-entry`).first().waitFor();
  await page.locator(`.inbox-project[data-project="${projectKey}"] .inbox-recent .inbox-quiet-task`).first().waitFor();
  await page.mouse.move(0, 0);
}

/** The active block's geometry and fills: head, Recent heading, and each row's
 *  title, facts and right-edge controls, in CSS pixels. */
export async function inspectActiveBlock(page) {
  return page.locator(".inbox-project.active").evaluate((block) => {
    const box = (element) => {
      if (!element) return null;
      const { x, right, width } = element.getBoundingClientRect();
      return { x, right, width };
    };
    const fill = (element) => getComputedStyle(element).backgroundColor;
    const resolved = (value) => {
      const probe = block.appendChild(document.createElement("span"));
      probe.style.background = value;
      const colour = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return colour;
    };
    const toggle = block.querySelector(".inbox-recent-toggle");
    const head = block.querySelector(".inbox-project-head");
    return {
      background: fill(block), head: box(head), headFill: fill(head), name: box(block.querySelector(".inbox-project-name")),
      create: box(block.querySelector(".inbox-project-create")),
      createGlyph: box(block.querySelector(".inbox-project-create svg")),
      toggle: box(toggle), toggleFill: fill(toggle), caret: box(toggle.querySelector(".inbox-recent-caret")),
      count: box(toggle.querySelector(".inbox-recent-count")),
      accentSoft: resolved("var(--accent-soft)"), panel: resolved("var(--panel)"), ground: resolved("var(--bg)"),
      rows: [...block.querySelectorAll(".inbox-entry")].map((row) => {
        const title = row.querySelector(".stitle");
        const titleBox = box(title);
        const facts = box(row.querySelector(".inbox-facts-float"));
        const middle = titleBox && document.elementFromPoint(titleBox.x + 2,
          title.getBoundingClientRect().top + title.getBoundingClientRect().height / 2);
        return { key: row.dataset.key, active: row.classList.contains("active"), row: box(row), fill: fill(row),
          inheritedFill: resolved(getComputedStyle(row).getPropertyValue("--inbox-row-fill")),
          actionsFill: fill(row.querySelector(".inbox-actions")), title: titleBox, facts,
          text: title.textContent, clipped: title.scrollWidth > title.clientWidth,
          at: Boolean(middle && title.contains(middle)),
          done: box(row.querySelector(".inbox-workspace-done")), watch: box(row.querySelector(".inbox-watch")),
          watchGlyph: box(row.querySelector(".inbox-watch svg")) };
      }),
    };
  });
}

/** Theme transitions must finish before inspecting a fill or capturing pixels. */
export async function settleTheme(page, theme) {
  await page.evaluate((theme) => {
    document.documentElement.dataset.theme = theme;
    for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
  }, theme);
}

/** Hover away from controls, including in the touch viewport's evidence. */
export async function hoverRecentRow(page) {
  const row = page.locator('.inbox-project.active [data-key="tracker_task:release-check"]');
  const box = await row.boundingBox();
  await page.mouse.move(box.x + 6, box.y + box.height / 2);
  await row.evaluate((element) => {
    for (const animation of element.getAnimations({ subtree: true })) if (animation instanceof CSSTransition) animation.finish();
  });
}

/** Exercise styles.css's imports without the harness's duplicate shell sheets. */
export async function useProductionStyles(page) {
  await page.evaluate(() => {
    for (const sheet of document.querySelectorAll('link[href$="src/styles/shell.css"], style[data-vite-dev-id$="/src/styles/shell.css"]')) sheet.remove();
  });
}
