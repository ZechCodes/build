/** @vitest-environment jsdom */
// #104: every watched task with unread wears a bubble wherever tasks are
// listed — the list, the board and the dashboard — and an unwatched one never
// does.
import { describe, expect, it } from "vitest";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { taskCardHtml } from "../src/core/trackerBoardRender.js";
import { paintTaskDashboard } from "../src/core/trackerDashboardRender.js";
import { columns, task } from "./trackerWireFixture.js";

const context = (over = {}) => ({ columns: columns(), agentLabels: {}, href: (one) => `#/tasks/${one.id}`, ...over });
const parse = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const watched = task({ id: "i-12", number: 12, watched: true, unread_count: 3 });
const unwatched = task({ id: "i-13", number: 13, unread_count: 3 });

function dashboard(one, over = {}) {
  const body = document.createElement("div");
  paintTaskDashboard(body, { needsYou: [{ task: one, reasonLabels: ["Assigned to you"] }] }, context({
    dashboardTab: "needsYou", onDashboardTab: () => {}, ...over,
  }));
  return body;
}

describe("the list", () => {
  it("puts the bubble on line one, after the title", () => {
    const row = parse(taskRowHtml(watched, context())).querySelector(".task-row");
    const lineOne = [...row.querySelector(".task-row-open").children].map((one) => one.classList[0]);
    expect(lineOne.slice(-2)).toEqual(["task-title", "badge"]);
    expect(row.querySelector(".task-row-open .task-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched task", () => {
    expect(parse(taskRowHtml(unwatched, context())).querySelector(".task-unread")).toBeNull();
  });

  it("says what the pane counted, which reads the cached timeline", () => {
    const row = parse(taskRowHtml(watched, context({ unreadOf: () => 0 })));
    expect(row.querySelector(".task-unread")).toBeNull();
  });
});

describe("the board", () => {
  it("puts the bubble in the card's head", () => {
    const card = parse(taskCardHtml(watched, context()));
    expect(card.querySelector(".task-card-head .task-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched task", () => {
    expect(parse(taskCardHtml(unwatched, context())).querySelector(".task-unread")).toBeNull();
  });
});

describe("the dashboard", () => {
  it("puts the bubble on the row", () => {
    expect(dashboard(watched).querySelector(".task-dashboard-link .task-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched task", () => {
    expect(dashboard(unwatched).querySelector(".task-unread")).toBeNull();
  });

  it("says what the pane counted", () => {
    expect(dashboard(watched, { unreadOf: () => 5 }).querySelector(".task-unread").textContent).toBe("5");
  });
});
