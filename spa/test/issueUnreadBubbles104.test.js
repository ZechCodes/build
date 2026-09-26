/** @vitest-environment jsdom */
// #104: every watched issue with unread wears a bubble wherever issues are
// listed — the list, the board and the dashboard — and an unwatched one never
// does.
import { describe, expect, it } from "vitest";
import { issueRowHtml } from "../src/core/trackerListRender.js";
import { issueCardHtml } from "../src/core/trackerBoardRender.js";
import { paintIssueDashboard } from "../src/core/trackerDashboardRender.js";
import { columns, issue } from "./trackerWireFixture.js";

const context = (over = {}) => ({ columns: columns(), agentLabels: {}, href: (one) => `#/issues/${one.id}`, ...over });
const parse = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const watched = issue({ id: "i-12", number: 12, watched: true, unread_count: 3 });
const unwatched = issue({ id: "i-13", number: 13, unread_count: 3 });

function dashboard(one, over = {}) {
  const body = document.createElement("div");
  paintIssueDashboard(body, { needsYou: [{ issue: one, reasonLabels: ["Assigned to you"] }] }, context({
    dashboardTab: "needsYou", onDashboardTab: () => {}, ...over,
  }));
  return body;
}

describe("the list", () => {
  it("puts the bubble on line one, after the title", () => {
    const row = parse(issueRowHtml(watched, context())).querySelector(".issue-row");
    const lineOne = [...row.querySelector(".issue-row-open").children].map((one) => one.classList[0]);
    expect(lineOne.slice(-2)).toEqual(["issue-title", "badge"]);
    expect(row.querySelector(".issue-row-open .issue-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched issue", () => {
    expect(parse(issueRowHtml(unwatched, context())).querySelector(".issue-unread")).toBeNull();
  });

  it("says what the pane counted, which reads the cached timeline", () => {
    const row = parse(issueRowHtml(watched, context({ unreadOf: () => 0 })));
    expect(row.querySelector(".issue-unread")).toBeNull();
  });
});

describe("the board", () => {
  it("puts the bubble in the card's head", () => {
    const card = parse(issueCardHtml(watched, context()));
    expect(card.querySelector(".issue-card-head .issue-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched issue", () => {
    expect(parse(issueCardHtml(unwatched, context())).querySelector(".issue-unread")).toBeNull();
  });
});

describe("the dashboard", () => {
  it("puts the bubble on the row", () => {
    expect(dashboard(watched).querySelector(".issue-dashboard-link .issue-unread").textContent).toBe("3");
  });

  it("wears none for an unwatched issue", () => {
    expect(dashboard(unwatched).querySelector(".issue-unread")).toBeNull();
  });

  it("says what the pane counted", () => {
    expect(dashboard(watched, { unreadOf: () => 5 }).querySelector(".issue-unread").textContent).toBe("5");
  });
});
