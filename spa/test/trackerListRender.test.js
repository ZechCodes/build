/** @vitest-environment jsdom */
// One row of the Issues tab, as #28 asks for it: the number and the title on
// the first line with nothing else beside them, and everything else on the
// second, in one order, spaced rather than punctuated.
//
// The row is parsed into a real DOM rather than matched as a string, because
// what this issue is about is ORDER and ABSENCE — and a substring test cannot
// say that the status chip comes before the age, nor that no separator sits
// between them.

import { describe, expect, it } from "vitest";
import { issueRowHtml } from "../src/core/trackerListRender.js";
import { paintIssueRows } from "../src/core/trackerIssuesBody.js";
import { DEFAULT_FILTERS } from "../src/core/trackerFilters.js";
import { columns, issue } from "./trackerWireFixture.js";

const NOW = Date.parse("2026-08-21T12:00:00Z");
const LABELS = { "agent-1": "issues-spa · Agent 1" };

const render = (over = {}) => {
  const host = document.createElement("div");
  host.innerHTML = issueRowHtml(issue({ number: 12, id: "issue-12", ...over }), {
    columns: columns(),
    agentLabels: LABELS,
    href: (one) => `#/issues/${one.id}`,
    nowMs: NOW,
  });
  return host.querySelector(".issue-row");
};

/** Every mark on the second line, in the order the row draws them, said as
 *  `class:text` so a case can assert the sequence and nothing else. */
const facts = (row) =>
  [...row.querySelector(".issue-row-facts").children].map(
    (one) => `${one.classList[0]}:${one.textContent.trim()}`,
  );

describe("the first line", () => {
  it("is the number and the title, and nothing else", () => {
    const row = render({ title: "Kanban drag does not persist", labels: ["bug"], priority: "high" });
    const line = row.querySelector(".issue-row-open");
    // Said as class:text, because the gap between the number and the title is
    // the stylesheet's and there is deliberately no whitespace node for it.
    expect([...line.children].map((one) => `${one.classList[0]}:${one.textContent}`))
      .toEqual(["issue-number:#12", "issue-title:Kanban drag does not persist"]);
  });

  it("is the link to the issue, so a middle-click and a copied address work", () => {
    expect(render().querySelector(".issue-row-open").getAttribute("href")).toBe("#/issues/issue-12");
  });
});

describe("the second line", () => {
  it("reads status, priority, age, labels, assignee — in that order", () => {
    const row = render({
      status: "in_review",
      priority: "high",
      labels: ["bug", "ui"],
      assignee: { kind: "agent", agent_id: "agent-1" },
      updated_at: "2026-08-21T10:00:00Z",
    });
    expect(facts(row)).toEqual([
      "issue-status:In review",
      "issue-priority:High",
      "issue-age:2h ago",
      "issue-label:bug",
      "issue-label:ui",
      "issue-assign:issues-spa · Agent 1",
    ]);
  });

  // `none` and `low` wear no chip: a list where every row has one says nothing
  // with it.
  it("leaves the priority out when there is none worth a mark", () => {
    expect(facts(render({ priority: "none" })).some((one) => one.startsWith("issue-priority"))).toBe(false);
    expect(facts(render({ priority: "low" })).some((one) => one.startsWith("issue-priority"))).toBe(false);
  });

  it("leaves the age out rather than guessing when the stamp says nothing", () => {
    expect(facts(render({ updated_at: null })).some((one) => one.startsWith("issue-age"))).toBe(false);
  });
});

// #33. The list opens on open issues, so a closed row is only ever on screen
// because the reader asked for one — and once it is there it must not read as
// open. The row lost its state dot with the dots (#28), so this is the mark.
describe("a closed issue, when the filter asked for one", () => {
  it("leads line two with a Closed chip", () => {
    const row = render({ state: "closed", status: "in_review", priority: "high" });
    expect(facts(row)[0]).toBe("issue-closed:Closed");
  });

  // "Open" on every other row is a word the reader learns to skip — the
  // mistake "Unassigned" made before it.
  it("says nothing at all on an open one", () => {
    const row = render({ state: "open" });
    expect(row.querySelector(".issue-closed")).toBeNull();
    expect(row.textContent).not.toContain("Open");
  });

  it("leaves the rest of line two in its order behind it", () => {
    const row = render({ state: "closed", status: "done", priority: "urgent", labels: ["bug"] });
    expect(facts(row).map((one) => one.split(":")[0])).toEqual([
      "issue-closed", "issue-status", "issue-priority", "issue-age", "issue-label", "issue-assign",
    ]);
  });
});

describe("no dots", () => {
  it("draws no state dot on the row", () => {
    expect(render({ state: "closed" }).querySelector(".issue-state")).toBeNull();
    expect(render({ state: "open" }).querySelector(".issue-state")).toBeNull();
  });

  it("puts no separator between the facts — the spacing is the separator", () => {
    const row = render({ priority: "urgent", labels: ["bug"], assignee: { kind: "user" } });
    expect(row.querySelector(".issue-sep")).toBeNull();
    expect(row.textContent).not.toContain("·".repeat(1) + " ");
  });

  // The one `·` left anywhere near a row is INSIDE an agent's name, which is
  // what tells one agent from another (core/trackerAssignee.js).
  it("keeps the dot inside an agent's own name", () => {
    const row = render({ assignee: { kind: "agent", agent_id: "agent-1" } });
    expect(row.querySelector(".issue-assign").textContent.trim()).toBe("issues-spa · Agent 1");
  });
});

describe("the assignee", () => {
  it("is the assignee's name when somebody holds it", () => {
    const row = render({ assignee: { kind: "user" } });
    expect(row.querySelector(".issue-assignee").textContent).toBe("You");
  });

  // "Unassigned" on every unheld row is a word the reader learns to skip.
  it("says nothing at all when nobody holds it", () => {
    const row = render({ assignee: null });
    expect(row.textContent).not.toContain("Unassigned");
    expect(row.querySelector(".issue-assignee")).toBeNull();
  });

  // Not nothing, though: the press is still there, and says what it does when
  // it is reached. The stylesheet is what keeps it quiet until then.
  it("offers a quiet Assign in its place, still a button and still reachable", () => {
    const button = render({ assignee: null }).querySelector(".issue-assign");
    expect(button.tagName).toBe("BUTTON");
    expect(button.querySelector(".issue-assign-cue").textContent).toBe("Assign");
    expect(button.getAttribute("aria-label")).toBe("Assign #12");
    expect(button.dataset.issueAssign).toBe("issue-12");
  });

  // A press whose visible words are not in its accessible name is a press a
  // speech-control user cannot say out loud (WCAG 2.5.3).
  it("names the holder in the accessible name too, so the two agree", () => {
    const button = render({ assignee: { kind: "agent", agent_id: "agent-1" } }).querySelector(".issue-assign");
    expect(button.getAttribute("aria-label")).toContain("issues-spa · Agent 1");
    expect(button.getAttribute("aria-label")).toContain("#12");
  });

  // It sits on line two with the other facts, not in a column of its own: a
  // button nested inside the row's own link would be a control inside a link.
  it("is a sibling of the row's link, never inside it", () => {
    const row = render({ assignee: { kind: "user" } });
    expect(row.querySelector(".issue-row-open .issue-assign")).toBeNull();
    expect(row.querySelector(".issue-row-facts .issue-assign")).not.toBeNull();
  });
});

describe("the list around the rows", () => {
  const paintInto = (host, issues, filters = DEFAULT_FILTERS) =>
    paintIssueRows(host, issues, {
      columns: columns(),
      agentLabels: {},
      href: (one) => `#/issues/${one.id}`,
      filters,
      nowMs: NOW,
    });

  const paint = (issues, filters) => {
    const host = document.createElement("div");
    paintInto(host, issues, filters);
    return host;
  };

  const numbers = (host) => [...host.querySelectorAll(".issue-number")].map((one) => one.textContent);

  it("draws one row per issue, in the order it is handed them", () => {
    expect(numbers(paint([issue({ number: 12, id: "i12" }), issue({ number: 11, id: "i11" })]))).toEqual(["#12", "#11"]);
  });

  // The rows are patched by key (core/patchList.js), the way the timeline is:
  // a row that is still there is still the same element after a paint, so the
  // press on it keeps the handler it was wired with and the keyboard keeps its
  // place. An insert above it is an insert, not a rebuild of everything below.
  it("keeps the element of a row that is still there when one is inserted above it", () => {
    const host = paint([issue({ number: 11, id: "i11" })]);
    const kept = host.querySelector('[data-issue="i11"]');
    paintInto(host, [issue({ number: 12, id: "i12" }), issue({ number: 11, id: "i11" })]);
    expect(numbers(host)).toEqual(["#12", "#11"]);
    expect(host.querySelector('[data-issue="i11"]')).toBe(kept);
  });

  it("wires each row it had to make, once, and never one it kept", () => {
    const wired = [];
    const wire = (element) => wired.push(element.dataset.issue);
    const host = document.createElement("div");
    paintIssueRows(host, [issue({ number: 11, id: "i11" })], { columns: columns(), agentLabels: {}, href: () => "#", filters: DEFAULT_FILTERS, nowMs: NOW }, wire);
    paintIssueRows(host, [issue({ number: 12, id: "i12" }), issue({ number: 11, id: "i11" })], { columns: columns(), agentLabels: {}, href: () => "#", filters: DEFAULT_FILTERS, nowMs: NOW }, wire);
    expect(wired).toEqual(["i11", "i12"]);
  });

  it("says the project is empty, and says a filter is why when one is set", () => {
    expect(paint([]).querySelector(".issue-empty h2").textContent).toBe("No issues yet");
    expect(paint([], { ...DEFAULT_FILTERS, state: "closed" }).querySelector(".issue-empty").textContent)
      .toContain("No issue matches these filters");
  });

  it("takes the empty line away once there is a row, and puts it back", () => {
    const host = paint([]);
    paintInto(host, [issue({ number: 12, id: "i12" })]);
    expect(host.querySelector(".issue-empty")).toBeNull();
    expect(numbers(host)).toEqual(["#12"]);
    paintInto(host, []);
    expect(host.querySelector(".issue-empty")).not.toBeNull();
    expect(numbers(host)).toEqual([]);
  });
});
