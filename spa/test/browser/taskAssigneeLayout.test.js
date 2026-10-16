import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const workspace = 'Board cards with a very long workspace name "overflow" and no room left ' + "workspace".repeat(8);
const agent = "An extraordinarily long agent name that must stay inside its card";
const fullName = `${workspace} · ${agent}`;

async function mountAssignments(page, basePath, columnWidth) {
  await mountLayout(page, '<main id="assignments"></main>', {
    basePath, styles: `@import url("${basePath}src/styles/tasks.css");
      body{display:block;padding:16px} #assignments{width:max-content;max-width:none}
      .task-column{flex-basis:${columnWidth}px} .task-rows{width:${columnWidth}px;margin-top:16px}`,
  });
  await loadBrowserModules(page, {
    board: "src/core/trackerBoardRender.js", list: "src/core/trackerListRender.js",
  }, basePath);
  await page.evaluate(({ workspace, agent }) => {
    const { board, list } = window.__layoutModules;
    document.documentElement.dataset.theme = "dark";
    const identities = { "agent-long": { agent_id: "agent-long", workspace_id: "ws-long",
      workspace_name: workspace, name: agent, provider: "codex_app_server", available: true } };
    const context = { deviceId: "layout", projectId: "p1", href: () => "#task",
      columns: [{ id: "in_progress", name: "In progress" }, { id: "in_review", name: "In review" }] };
    const task = { id: "task-long", number: 632, title: "Keep the assignee inside the card",
      status: "in_progress", state: "open", priority: "medium", labels: [],
      assignee: { kind: "agent", agent_id: "agent-long" }, identities };
    const tasks = [task,
      { ...task, id: "task-review", number: 86, status: "in_review", review_summary: { status: "open" } },
      { ...task, id: "task-unavailable", number: 630,
        identities: { "agent-long": { ...identities["agent-long"], available: false } } },
      { ...task, id: "task-user", number: 628, assignee: { kind: "user" } },
      { ...task, id: "task-unassigned", number: 631, assignee: null }];
    const columns = context.columns.map((column) => ({ ...column, tasks: tasks.filter((task) => task.status === column.id) }));
    const root = document.querySelector("#assignments");
    root.innerHTML = board.boardFrameHtml(columns, context);
    for (const column of columns) {
      root.querySelector(`[data-column-drop="${column.id}"]`).innerHTML = column.tasks.map((task) => board.taskCardHtml(task, context)).join("");
    }
    root.insertAdjacentHTML("beforeend", `<ul class="task-rows">${tasks.map((task) => list.taskRowHtml(task, context)).join("")}</ul>`);
  }, { workspace, agent });
}

async function assignmentMeasurements(page) {
  return page.locator(".task-card, .task-row").evaluateAll((cards) => cards.map((card) => {
    const bounds = card.getBoundingClientRect();
    const box = (element) => element?.getBoundingClientRect().toJSON();
    const entry = card.querySelector(".task-assignee-entry") || card.querySelector(".task-assign");
    const name = card.querySelector(".task-assignee-name");
    const link = card.querySelector(".task-assignee-link");
    const range = document.createRange();
    if (name) range.selectNodeContents(name);
    const lines = [...range.getClientRects()].map((line) => line.toJSON());
    return {
      id: `${card.className}:${card.dataset.task}`, bounds: bounds.toJSON(),
      outside: [...card.querySelectorAll("*")].filter((child) => {
        const rect = child.getBoundingClientRect();
        return rect.width && rect.height && (rect.left < bounds.left - 0.5 || rect.right > bounds.right + 0.5 ||
          rect.top < bounds.top - 0.5 || rect.bottom > bounds.bottom + 0.5);
      }).map((child) => child.className.baseVal ?? child.className),
      title: link?.title, link: box(link), icon: box(card.querySelector(".task-avatar")),
      change: box(card.querySelector(".task-assignee-entry .task-assign")), entry: box(entry),
      previous: box(entry?.previousElementSibling), status: box(card.querySelector(".task-status")),
      name: name && { ...box(name), text: name.textContent, clipped: name.scrollWidth > name.clientWidth,
        verticallyClipped: name.scrollHeight > name.clientHeight,
        lines,
        overflow: getComputedStyle(name).textOverflow, whitespace: getComputedStyle(name).whiteSpace },
    };
  }));
}

function assertNameLayout(card, phone) {
  expect(card.name.text).toBe(fullName);
  expect(card.name.overflow).toBe(phone ? "clip" : "ellipsis");
  expect(card.name.whitespace).toBe(phone ? "normal" : "nowrap");
  if (phone) {
    expect(card.name.lines.length).toBeGreaterThan(1);
    expect(Math.abs(card.icon.top - card.name.lines[0].top), card.id).toBeLessThanOrEqual(4);
    expect(Math.min(...card.name.lines.slice(1).map((line) => line.left)), card.id)
      .toBeLessThanOrEqual(card.icon.left + 0.5);
    for (const line of card.name.lines) {
      expect(line.left, card.id).toBeGreaterThanOrEqual(card.bounds.left);
      expect(line.right, card.id).toBeLessThanOrEqual(card.bounds.right);
      expect(line.bottom, card.id).toBeLessThanOrEqual(card.bounds.bottom);
    }
  } else {
    expect(card.name.clipped, card.id).toBe(true);
    expect(card.name.verticallyClipped, card.id).toBe(false);
    expect(card.name.height).toBeLessThan(20);
  }
  expect(card.icon.width).toBe(16);
  expect(card.icon.right).toBeLessThanOrEqual(card.name.lines[0].left);
}

for (const { width, columnWidth } of [
  { width: 1280, columnWidth: 180 }, { width: 1280, columnWidth: 260 },
  { width: 1280, columnWidth: 360 }, { width: 390, columnWidth: 260 },
  { width: 320, columnWidth: 260 }, { width: 760, columnWidth: 260 },
  { width: 761, columnWidth: 260 },
]) {
  it(`contains long assignees in ${columnWidth}px board columns and list rows at ${width}px (#632)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountAssignments(page, basePath, columnWidth);
      await captureLayout(page, `task-assignee-${width}-${columnWidth}.png`);
      const measurements = await assignmentMeasurements(page);
      for (const card of measurements) {
        expect(card.outside, card.id).toEqual([]);
        if (card.link || card.id.endsWith(":task-unavailable")) {
          assertNameLayout(card, width <= 760);
        }
        if (card.link) {
          expect(card.title).toBe(fullName);
          expect(card.link.right).toBeLessThanOrEqual(card.change.left);
          expect(card.change.width).toBeGreaterThan(30);
        }
        if (card.id.startsWith("task-card") && card.previous) {
          expect(card.previous.bottom, card.id).toBeLessThanOrEqual(card.entry.top);
        }
      }
    }, { width, height: width > 760 ? 1100 : 1800 });
  });
}

it("shows an unavailable agent's full name on hover while preserving the assignment action (#632)", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountAssignments(page, basePath, 260);
    for (const container of [".task-card", ".task-row"]) {
      const button = page.locator(`${container}[data-task="task-unavailable"] .task-assign`);
      expect(await button.locator(".task-assignee-name").getAttribute("title")).toBe(fullName);
      expect(await button.getAttribute("title")).toBe("Assign this task");
      expect(await button.getAttribute("aria-label")).toBe(container === ".task-card"
        ? "Assign #630" : `#630 is assigned to ${fullName}. Assign it to somebody else`);
    }
  });
});

for (const task of ["task-user", "task-unassigned"]) {
  it(`preserves the assignment action tooltip for ${task} on Board and List (#632)`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountAssignments(page, basePath, 260);
      for (const container of [".task-card", ".task-row"]) {
        const button = page.locator(`${container}[data-task="${task}"] .task-assign`);
        expect(await button.getAttribute("title")).toBe("Assign this task");
        expect(await button.locator("[title]").count()).toBe(0);
        const hoveredTitles = await button.evaluate((button) =>
          [...button.querySelectorAll(".task-assignee-name, .task-assign-cue")]
            .map((name) => name.closest("[title]")?.title));
        expect(hoveredTitles).toEqual(["Assign this task"]);
      }
    });
  });
}
