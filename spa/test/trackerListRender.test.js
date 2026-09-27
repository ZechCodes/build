/** @vitest-environment jsdom */
// One row of the Tasks tab, as #28 asks for it: the number and the title on
// the first line with nothing else beside them, and everything else on the
// second, in one order, spaced rather than punctuated.
//
// The row is parsed into a real DOM rather than matched as a string, because
// what this task is about is ORDER and ABSENCE — and a substring test cannot
// say that the status chip comes before the age, nor that no separator sits
// between them.

import { describe, expect, it } from "vitest";
import { taskRowHtml } from "../src/core/trackerListRender.js";
import { paintTaskRows } from "../src/core/trackerTasksBody.js";
import { DEFAULT_FILTERS } from "../src/core/trackerFilters.js";
import { columns, task } from "./trackerWireFixture.js";

const NOW = Date.parse("2026-08-21T12:00:00Z");
const LABELS = { "agent-1": "tasks-spa · Agent 1" };

const render = (over = {}) => {
  const host = document.createElement("div");
  host.innerHTML = taskRowHtml(task({ number: 12, id: "task-12", ...over }), {
    columns: columns(),
    agentLabels: LABELS,
    href: (one) => `#/tasks/${one.id}`,
    nowMs: NOW,
  });
  return host.querySelector(".task-row");
};

/** Every mark on the second line, in the order the row draws them, said as
 *  `class:text` so a case can assert the sequence and nothing else. */
const facts = (row) =>
  [...row.querySelector(".task-row-facts").children].map(
    (one) => `${one.classList[0]}:${one.textContent.trim()}`,
  );

/** Every element on line one, said the same way. */
const lineOne = (row) =>
  [...row.querySelector(".task-row-open").children].map((one) => `${one.classList[0]}:${one.textContent}`);

describe("the first line", () => {
  it("is the number and the title, and nothing else", () => {
    const row = render({ title: "Kanban drag does not persist", labels: ["bug"], priority: "medium" });
    // Said as class:text, because the gap between the number and the title is
    // the stylesheet's and there is deliberately no whitespace node for it.
    expect(lineOne(row)).toEqual(["task-number:#12", "task-title:Kanban drag does not persist"]);
  });

  // #45: the priority is a mark before the title rather than a chip on line
  // two — it says how to READ the title, and the eye going down a column of
  // titles meets it on the way in.
  it("puts a pressing priority between the number and the title", () => {
    expect(lineOne(render({ title: "Fix it", priority: "high" })))
      .toEqual(["task-number:#12", "task-priority-mark:!", "task-title:Fix it"]);
    expect(lineOne(render({ title: "Fix it", priority: "urgent" })))
      .toEqual(["task-number:#12", "task-priority-mark:!!", "task-title:Fix it"]);
  });

  // Shape as well as colour: a mark that is only a colour is not a mark to
  // every reader, and a name for the one who cannot see it at all.
  it("names the priority where the mark cannot be seen", () => {
    const mark = render({ priority: "urgent" }).querySelector(".task-priority-mark");
    expect(mark.getAttribute("aria-label")).toBe("Urgent priority");
    expect(mark.classList.contains("task-priority-mark-urgent")).toBe(true);
  });

  // Only high and urgent. A list where most rows carry a mark has no marks in
  // it, so medium — the ordinary case — says nothing here, even though the
  // board card still chips it.
  it("marks nothing at medium and below", () => {
    for (const priority of ["none", "low", "medium"]) {
      expect(render({ priority }).querySelector(".task-priority-mark")).toBeNull();
    }
  });

  it("is the link to the task, so a middle-click and a copied address work", () => {
    expect(render().querySelector(".task-row-open").getAttribute("href")).toBe("#/tasks/task-12");
  });
});

describe("the second line", () => {
  // The same facts in the same order as #28 asked for them, minus the priority
  // chip, which went to line one (#45). The column is the one chip left.
  it("reads status, age, labels, assignee — in that order", () => {
    const row = render({
      status: "in_review",
      priority: "high",
      labels: ["bug", "ui"],
      assignee: { kind: "agent", agent_id: "agent-1" },
      updated_at: "2026-08-21T10:00:00Z",
    });
    expect(facts(row)).toEqual([
      "task-status:In review · tasks-spa · Agent 1", // who it is with (#144)
      "task-age:2h ago",
      "task-labels:bugui", // two label words, spaced by the stylesheet and nothing else
      "task-assign:tasks-spa · Agent 1",
    ]);
  });

  it("leaves the priority chip off the row entirely", () => {
    for (const priority of ["none", "low", "medium", "high", "urgent"]) {
      expect(render({ priority }).querySelector(".task-priority")).toBeNull();
    }
  });

  // #45: labels are small muted words rather than pills, and a row shows three
  // of them. The rest are a count, and the count names them rather than losing
  // them — a row wearing six labels used to be a wall of six boxes.
  it("shows three labels and counts the rest", () => {
    const row = render({ labels: ["bug", "ui", "spa", "tracker", "perf"] });
    const labels = [...row.querySelectorAll(".task-labels .task-label")].map((one) => one.textContent);
    expect(labels).toEqual(["bug", "ui", "spa", "+2"]);
    expect(row.querySelector(".task-label-more").getAttribute("title")).toBe("tracker, perf");
  });

  it("counts nothing when three is all there is", () => {
    const row = render({ labels: ["bug", "ui", "spa"] });
    expect(row.querySelector(".task-label-more")).toBeNull();
  });

  it("draws no label group at all on a task wearing none", () => {
    expect(render({ labels: [] }).querySelector(".task-labels")).toBeNull();
  });

  it("leaves the age out rather than guessing when the stamp says nothing", () => {
    expect(facts(render({ updated_at: null })).some((one) => one.startsWith("task-age"))).toBe(false);
  });
});

// #33. The list opens on open tasks, so a closed row is only ever on screen
// because the reader asked for one — and once it is there it must not read as
// open. The row lost its state dot with the dots (#28), so this is the mark.
describe("a closed task, when the filter asked for one", () => {
  it("leads line two with a Closed chip", () => {
    const row = render({ state: "closed", status: "in_review", priority: "high" });
    expect(facts(row)[0]).toBe("task-closed:Closed");
  });

  // "Open" on every other row is a word the reader learns to skip — the
  // mistake "Unassigned" made before it.
  it("says nothing at all on an open one", () => {
    const row = render({ state: "open" });
    expect(row.querySelector(".task-closed")).toBeNull();
    expect(row.textContent).not.toContain("Open");
  });

  it("leaves the rest of line two in its order behind it", () => {
    const row = render({ state: "closed", status: "done", priority: "urgent", labels: ["bug"] });
    expect(facts(row).map((one) => one.split(":")[0])).toEqual([
      "task-closed", "task-status", "task-age", "task-labels", "task-assign",
    ]);
  });
});

describe("no dots", () => {
  it("draws no state dot on the row", () => {
    expect(render({ state: "closed" }).querySelector(".task-state")).toBeNull();
    expect(render({ state: "open" }).querySelector(".task-state")).toBeNull();
  });

  it("puts no separator between the facts — the spacing is the separator", () => {
    const row = render({ priority: "urgent", labels: ["bug"], assignee: { kind: "user" } });
    expect(row.querySelector(".task-sep")).toBeNull();
    expect(row.textContent).not.toContain("·".repeat(1) + " ");
  });

  // The one `·` left anywhere near a row is INSIDE an agent's name, which is
  // what tells one agent from another (core/trackerAssignee.js).
  it("keeps the dot inside an agent's own name", () => {
    const row = render({ assignee: { kind: "agent", agent_id: "agent-1" } });
    expect(row.querySelector(".task-assign").textContent.trim()).toBe("tasks-spa · Agent 1");
  });
});

describe("the assignee", () => {
  it("is the assignee's name when somebody holds it", () => {
    const row = render({ assignee: { kind: "user" } });
    expect(row.querySelector(".task-assignee").textContent).toBe("You");
  });

  // "Unassigned" on every unheld row is a word the reader learns to skip.
  it("says nothing at all when nobody holds it", () => {
    const row = render({ assignee: null });
    expect(row.textContent).not.toContain("Unassigned");
    expect(row.querySelector(".task-assignee")).toBeNull();
  });

  // Not nothing, though: the press is still there, and says what it does when
  // it is reached. The stylesheet is what keeps it quiet until then.
  it("offers a quiet Assign in its place, still a button and still reachable", () => {
    const button = render({ assignee: null }).querySelector(".task-assign");
    expect(button.tagName).toBe("BUTTON");
    expect(button.querySelector(".task-assign-cue").textContent).toBe("Assign");
    expect(button.getAttribute("aria-label")).toBe("Assign #12");
    expect(button.dataset.taskAssign).toBe("task-12");
  });

  // A press whose visible words are not in its accessible name is a press a
  // speech-control user cannot say out loud (WCAG 2.5.3).
  it("names the holder in the accessible name too, so the two agree", () => {
    const button = render({ assignee: { kind: "agent", agent_id: "agent-1" } }).querySelector(".task-assign");
    expect(button.getAttribute("aria-label")).toContain("tasks-spa · Agent 1");
    expect(button.getAttribute("aria-label")).toContain("#12");
  });

  // It sits on line two with the other facts, not in a column of its own: a
  // button nested inside the row's own link would be a control inside a link.
  it("is a sibling of the row's link, never inside it", () => {
    const row = render({ assignee: { kind: "user" } });
    expect(row.querySelector(".task-row-open .task-assign")).toBeNull();
    expect(row.querySelector(".task-row-facts .task-assign")).not.toBeNull();
  });
});

describe("the list around the rows", () => {
  const paintInto = (host, tasks, filters = DEFAULT_FILTERS) =>
    paintTaskRows(host, tasks, {
      columns: columns(),
      agentLabels: {},
      href: (one) => `#/tasks/${one.id}`,
      filters,
      nowMs: NOW,
    });

  const paint = (tasks, filters) => {
    const host = document.createElement("div");
    paintInto(host, tasks, filters);
    return host;
  };

  const numbers = (host) => [...host.querySelectorAll(".task-number")].map((one) => one.textContent);

  it("draws one row per task, in the order it is handed them", () => {
    expect(numbers(paint([task({ number: 12, id: "i12" }), task({ number: 11, id: "i11" })]))).toEqual(["#12", "#11"]);
  });

  // The rows are patched by key (core/patchList.js), the way the timeline is:
  // a row that is still there is still the same element after a paint, so the
  // press on it keeps the handler it was wired with and the keyboard keeps its
  // place. An insert above it is an insert, not a rebuild of everything below.
  it("keeps the element of a row that is still there when one is inserted above it", () => {
    const host = paint([task({ number: 11, id: "i11" })]);
    const kept = host.querySelector('[data-task="i11"]');
    paintInto(host, [task({ number: 12, id: "i12" }), task({ number: 11, id: "i11" })]);
    expect(numbers(host)).toEqual(["#12", "#11"]);
    expect(host.querySelector('[data-task="i11"]')).toBe(kept);
  });

  it("wires each row it had to make, once, and never one it kept", () => {
    const wired = [];
    const wire = (element) => wired.push(element.dataset.task);
    const host = document.createElement("div");
    paintTaskRows(host, [task({ number: 11, id: "i11" })], { columns: columns(), agentLabels: {}, href: () => "#", filters: DEFAULT_FILTERS, nowMs: NOW }, wire);
    paintTaskRows(host, [task({ number: 12, id: "i12" }), task({ number: 11, id: "i11" })], { columns: columns(), agentLabels: {}, href: () => "#", filters: DEFAULT_FILTERS, nowMs: NOW }, wire);
    expect(wired).toEqual(["i11", "i12"]);
  });

  it("says the project is empty, and says a filter is why when one is set", () => {
    expect(paint([]).querySelector(".task-empty h2").textContent).toBe("No tasks yet");
    expect(paint([], { ...DEFAULT_FILTERS, state: "closed" }).querySelector(".task-empty").textContent)
      .toContain("No task matches these filters");
  });

  it("takes the empty line away once there is a row, and puts it back", () => {
    const host = paint([]);
    paintInto(host, [task({ number: 12, id: "i12" })]);
    expect(host.querySelector(".task-empty")).toBeNull();
    expect(numbers(host)).toEqual(["#12"]);
    paintInto(host, []);
    expect(host.querySelector(".task-empty")).not.toBeNull();
    expect(numbers(host)).toEqual([]);
  });
});
