/** @vitest-environment jsdom */
// #144: an issue in review is assigned to whoever is reviewing it, and its
// card and row say who that is — "In review · you" or the reviewer's name —
// from the cached assignee alone.

import { describe, expect, it } from "vitest";
import { issueRowHtml } from "../src/core/trackerListRender.js";
import { issueCardHtml } from "../src/core/trackerBoardRender.js";
import { columns, issue } from "./trackerWireFixture.js";

const LABELS = { "agent-astra": "review · Astra reviewer" };
const context = { columns: columns(), agentLabels: LABELS, href: (one) => `#/issues/${one.id}` };
const astra = { kind: "agent", agent_id: "agent-astra" };

const parsed = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.firstElementChild;
};
const rowStatus = (over) => parsed(issueRowHtml(issue(over), context)).querySelector(".issue-status").textContent;
const cardStatus = (over) =>
  parsed(issueCardHtml(issue(over), context)).querySelector(".issue-card-status")?.textContent.trim() ?? null;

describe("who an issue in review is with", () => {
  it("names the reviewing agent, or the user as you, on the list row", () => {
    expect(rowStatus({ status: "in_review", assignee: astra })).toBe("In review · review · Astra reviewer");
    expect(rowStatus({ status: "in_review", assignee: { kind: "user" } })).toBe("In review · you");
  });

  it("says the same on the board card", () => {
    expect(cardStatus({ status: "in_review", assignee: astra })).toBe("In review · review · Astra reviewer");
    expect(cardStatus({ status: "in_review", assignee: { kind: "user" } })).toBe("In review · you");
  });

  it("says nothing more for an unheld review or any other column", () => {
    expect(rowStatus({ status: "in_review", assignee: null })).toBe("In review");
    expect(rowStatus({ status: "in_progress", assignee: { kind: "user" } })).toBe("In progress");
    expect(cardStatus({ status: "in_review", assignee: null })).toBe(null);
    expect(cardStatus({ status: "in_progress", assignee: astra })).toBe(null);
  });
});
