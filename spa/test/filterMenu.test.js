// The filter menu's decisions (#44): what a press changes, what a search
// leaves, where an arrow key lands, and what the press above it says.
//
// No DOM here on purpose — these are the answers the control draws, and a case
// that has to open a popover to ask "is two labels said as a count" is a case
// about the popover.

import { describe, expect, it } from "vitest";
import {
  emptyOption,
  firstActive,
  menuPressLabel,
  menuRows,
  moveActive,
  searchLabel,
  toggleChoice,
} from "../src/core/filterMenu.js";

const LABELS = [
  { value: "", label: "Any label" },
  { value: "bug", label: "bug" },
  { value: "tracker", label: "tracker" },
  { value: "transport", label: "transport" },
];

const ASSIGNEES = [
  { value: "", label: "Anyone" },
  { value: "any", label: "Anyone assigned" },
  { value: "none", label: "Unassigned" },
  { value: "user", label: "You" },
  { value: "agent:a1", label: "issues-spa · Agent 1", group: "issues-spa" },
  { value: "agent:a2", label: "issues-spa · Agent 2", group: "issues-spa" },
  { value: "agent:b1", label: "tracker-filters · Agent 1", group: "tracker-filters" },
];

const values = (rows) => rows.filter((row) => row.kind === "option").map((row) => row.value);

describe("what a press changes", () => {
  it("adds and removes in a multi menu, keeping the order they were ticked", () => {
    let chosen = toggleChoice([], "bug", { multi: true });
    chosen = toggleChoice(chosen, "tracker", { multi: true });
    expect(chosen).toEqual(["bug", "tracker"]);
    expect(toggleChoice(chosen, "bug", { multi: true })).toEqual(["tracker"]);
  });

  it("replaces in a single menu", () => {
    expect(toggleChoice(["open"], "closed", {})).toEqual(["closed"]);
  });

  // "Anyone" is not somebody: choosing the empty row empties the selection
  // rather than making `""` a chosen thing downstream.
  it("empties the selection when the empty row is pressed", () => {
    expect(toggleChoice(["closed"], "", {})).toEqual([]);
  });

  it("never edits the list it was handed", () => {
    const held = ["bug"];
    toggleChoice(held, "tracker", { multi: true });
    expect(held).toEqual(["bug"]);
  });
});

describe("what a search leaves", () => {
  // core/fuzzy.js, unchanged — a second ranker would be a second answer to one
  // question. So "tr" finds both `tracker` and `transport`, best first.
  it("ranks by subsequence, not by contains", () => {
    expect(values(menuRows(LABELS, "tr", [], { multi: true }))).toEqual(["tracker", "transport"]);
    expect(values(menuRows(LABELS, "tspt", [], { multi: true }))).toEqual(["transport"]);
  });

  it("leaves the caller's order alone when nothing is typed", () => {
    expect(values(menuRows(LABELS, "", [], { multi: true }))).toEqual(["bug", "tracker", "transport"]);
  });

  it("says nothing matched by leaving no rows", () => {
    expect(menuRows(LABELS, "zzz", [], { multi: true })).toEqual([]);
  });

  // A multi menu's empty selection already means "any", so the row that says
  // so is not on offer; a single menu's is a real answer among the others.
  it("drops the empty row from a multi menu and keeps it in a single one", () => {
    expect(values(menuRows(LABELS, "", [], { multi: true }))).not.toContain("");
    expect(values(menuRows(LABELS, "", [], {}))).toContain("");
  });

  it("marks what is chosen", () => {
    const rows = menuRows(LABELS, "", ["tracker"], { multi: true });
    expect(rows.filter((row) => row.checked).map((row) => row.value)).toEqual(["tracker"]);
  });

  it("writes a heading whenever the group changes, and none for the ungrouped", () => {
    const rows = menuRows(ASSIGNEES, "", [], { multi: true });
    expect(rows.map((row) => `${row.kind}:${row.kind === "group" ? row.label : row.value}`)).toEqual([
      "option:any", "option:none", "option:user",
      "group:issues-spa", "option:agent:a1", "option:agent:a2",
      "group:tracker-filters", "option:agent:b1",
    ]);
  });
});

describe("where an arrow key lands", () => {
  const rows = menuRows(ASSIGNEES, "", [], { multi: true });

  it("steps over the headings", () => {
    const first = firstActive(rows);
    expect(rows[first].value).toBe("any");
    const third = moveActive(rows, moveActive(rows, first, 1), 1);
    expect(rows[moveActive(rows, third, 1)].value).toBe("agent:a1");
  });

  // A held arrow key that runs off the bottom and reappears at the top has
  // lost the reader their place in a list they were reading.
  it("stops at each end rather than wrapping", () => {
    const first = firstActive(rows);
    expect(moveActive(rows, first, -1)).toBe(first);
    let last = first;
    for (let i = 0; i < 20; i += 1) last = moveActive(rows, last, 1);
    expect(rows[last].value).toBe("agent:b1");
    expect(moveActive(rows, last, 1)).toBe(last);
  });

  it("lands on the first option from nowhere, and the last one going up", () => {
    expect(rows[moveActive(rows, -1, 1)].value).toBe("any");
    expect(rows[moveActive(rows, -1, -1)].value).toBe("agent:b1");
  });

  it("has nowhere to land in an empty list", () => {
    expect(moveActive([], -1, 1)).toBe(-1);
    expect(firstActive([])).toBe(-1);
  });
});

describe("what the press says", () => {
  it("says the empty row's own words while nothing is chosen", () => {
    expect(menuPressLabel({ name: "Labels", options: LABELS, chosen: [] })).toBe("Any label");
    expect(menuPressLabel({ name: "Assignee", options: ASSIGNEES, chosen: [] })).toBe("Anyone");
  });

  it("names one chosen thing in full", () => {
    expect(menuPressLabel({ name: "Assignee", options: ASSIGNEES, chosen: ["agent:a1"] }))
      .toBe("issues-spa · Agent 1");
  });

  // Two kinds of name want different things: labels are short and
  // interchangeable, an assignee is somebody.
  it("counts several labels and leads with the first of several assignees", () => {
    expect(menuPressLabel({ name: "Labels", options: LABELS, chosen: ["bug", "tracker"], summary: "count" }))
      .toBe("Labels · 2");
    expect(menuPressLabel({ name: "Assignee", options: ASSIGNEES, chosen: ["agent:a1", "user"], summary: "first" }))
      .toBe("issues-spa · Agent 1 +1");
  });

  it("falls back to the value when nothing on offer names it", () => {
    expect(menuPressLabel({ name: "Labels", options: LABELS, chosen: ["gone"] })).toBe("gone");
  });

  it("falls back to the filter's own name when there is no empty row", () => {
    expect(menuPressLabel({ name: "Labels", options: [{ value: "bug", label: "bug" }], chosen: [] })).toBe("Labels");
  });
});

describe("the small words", () => {
  it("finds the row that means 'do not narrow on this'", () => {
    expect(emptyOption(LABELS).label).toBe("Any label");
    expect(emptyOption([{ value: "bug", label: "bug" }])).toBeNull();
  });

  it("names the search box out loud, because a placeholder is not a name", () => {
    expect(searchLabel("Labels")).toBe("Search labels");
  });
});
