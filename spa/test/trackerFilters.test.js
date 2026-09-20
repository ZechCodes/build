// The Issues tab's filters. They match `issues.list`'s params one for one — a
// filter is a param, not a client-side pass over everything — and the same
// narrowing is applied locally to the cached list while the read is in flight.

import { describe, expect, it } from "vitest";
import {
  ANY_ASSIGNEE,
  DEFAULT_FILTERS,
  NO_FILTERS,
  assigneesOf,
  filterIssues,
  filterOptions,
  filtersAreSet,
  issueListParams,
  labelsOf,
  narrowsTheRead,
  sortIssues,
} from "../src/core/trackerFilters.js";

const issue = (over = {}) => ({
  id: `issue-${over.number || 1}`,
  number: 1,
  title: "Kanban drag does not persist",
  state: "open",
  status: "backlog",
  labels: [],
  priority: "none",
  assignee: null,
  ...over,
});

describe("filters as issues.list params", () => {
  it("sends nothing but the project when nothing is narrowed", () => {
    expect(issueListParams("proj-1", NO_FILTERS)).toEqual({ project_id: "proj-1" });
    expect(issueListParams("proj-1")).toEqual({ project_id: "proj-1" });
  });

  it("sends each filter under the verb's own param name", () => {
    expect(issueListParams("proj-1", { state: "open", status: "in_review", label: "bug" })).toEqual({
      project_id: "proj-1", state: "open", status: "in_review", label: "bug",
    });
  });

  // The verb takes the actor shape, plus the two words.
  it("sends an assignee as the tagged shape, and the two words as words", () => {
    expect(issueListParams("proj-1", { assignee: "agent:agent-7" }).assignee).toEqual({
      kind: "agent", agent_id: "agent-7",
    });
    expect(issueListParams("proj-1", { assignee: "user" }).assignee).toEqual({ kind: "user" });
    expect(issueListParams("proj-1", { assignee: "none" }).assignee).toBe("none");
    expect(issueListParams("proj-1", { assignee: ANY_ASSIGNEE }).assignee).toBe("any");
  });

  // Absent means "do not narrow on this"; an empty string is not that.
  it("leaves an unset filter off the params rather than sending it empty", () => {
    expect(Object.keys(issueListParams("proj-1", { state: "", status: "", assignee: "", label: "" }))).toEqual([
      "project_id",
    ]);
  });
});

describe("the same narrowing, over the cached list", () => {
  const issues = [
    issue({ number: 3, state: "closed", status: "done", labels: ["bug"], assignee: { kind: "user" } }),
    issue({ number: 2, status: "in_progress", labels: ["bug", "ui"], assignee: { kind: "agent", agent_id: "agent-7" } }),
    issue({ number: 1 }),
  ];

  it("narrows on each filter the way the verb does", () => {
    expect(filterIssues(issues, { state: "closed" }).map((one) => one.number)).toEqual([3]);
    expect(filterIssues(issues, { status: "in_progress" }).map((one) => one.number)).toEqual([2]);
    expect(filterIssues(issues, { label: "ui" }).map((one) => one.number)).toEqual([2]);
    expect(filterIssues(issues, { assignee: "agent:agent-7" }).map((one) => one.number)).toEqual([2]);
  });

  it("ANDs them", () => {
    expect(filterIssues(issues, { state: "open", label: "bug" }).map((one) => one.number)).toEqual([2]);
  });

  it("reads the two assignee words the way the verb reads them", () => {
    expect(filterIssues(issues, { assignee: "none" }).map((one) => one.number)).toEqual([1]);
    expect(filterIssues(issues, { assignee: ANY_ASSIGNEE }).map((one) => one.number)).toEqual([3, 2]);
  });

  it("keeps the order it was given", () => {
    expect(filterIssues(issues, {}).map((one) => one.number)).toEqual([3, 2, 1]);
  });

  // Which tells "nothing here yet" from "nothing matches what you asked for".
  // Narrowed means narrowed beyond what the tab opens on (#33), so asking for
  // everything — closed issues included — is itself a narrowing.
  it("says whether anything is narrowed", () => {
    expect(filtersAreSet(DEFAULT_FILTERS)).toBe(false);
    expect(filtersAreSet({ ...DEFAULT_FILTERS, label: "bug" })).toBe(true);
    expect(filtersAreSet(NO_FILTERS)).toBe(true);
  });
});

describe("what the bar offers", () => {
  const issues = [
    issue({ number: 2, labels: ["ui", "bug"], assignee: { kind: "agent", agent_id: "agent-7" } }),
    issue({ number: 1, labels: ["bug"], assignee: { kind: "user" } }),
  ];

  it("offers every label the project's issues wear, alphabetically and once", () => {
    expect(labelsOf(issues)).toEqual(["bug", "ui"]);
  });

  it("offers every assignee they carry, once, and never the unassigned ones", () => {
    expect(assigneesOf([...issues, issue({ number: 3 })]).map((one) => one.value)).toEqual(["agent:agent-7", "user"]);
  });

  // A filter is a question about the project, not a summary of it: every
  // column is offered whether or not an issue stands in it.
  it("offers every column, including the empty ones", () => {
    const options = filterOptions([], null);
    expect(options.statuses.map((one) => one.value)).toEqual([
      "", "backlog", "ready", "in_progress", "in_review", "done",
    ]);
  });

  // #44: You is a standing answer now, beside Unassigned — it is a filter
  // anybody wants whether or not they happen to hold an issue today. An
  // assignee the issues name and the project does not is offered after them,
  // under a heading that says why it is odd.
  it("names each assignee through the caller, which knows this project's agents", () => {
    const options = filterOptions(issues, null, () => "Agent 1");
    expect(options.assignees.map((one) => one.label)).toEqual([
      "Anyone", "Anyone assigned", "Unassigned", "You", "Agent 1",
    ]);
    expect(options.assignees.at(-1).group).toBe("No longer in this project");
  });

  // The picker groups agents by the workspace they stand on, and so does this:
  // one agent is found the same way wherever it is chosen.
  it("offers the project's agents under the workspace each stands on", () => {
    const groups = [
      { workspaceId: "ws-1", name: "issues-spa", agents: [{ id: "agent-1", label: "issues-spa · Agent 1" }] },
      { workspaceId: "ws-2", name: "tracker-filters", agents: [{ id: "agent-2", label: "tracker-filters · Agent 1" }] },
    ];
    const offered = filterOptions([], null, () => "", groups).assignees;
    expect(offered.map((one) => [one.value, one.group || ""])).toEqual([
      ["", ""], ["any", ""], ["none", ""], ["user", ""],
      ["agent:agent-1", "issues-spa"], ["agent:agent-2", "tracker-filters"],
    ]);
  });

  // An agent that both stands on a workspace and holds an issue is one agent.
  it("offers each assignee once", () => {
    const groups = [{ workspaceId: "ws-1", name: "issues-spa", agents: [{ id: "agent-1", label: "issues-spa · Agent 1" }] }];
    const held = [issue({ assignee: { kind: "agent", agent_id: "agent-1" } })];
    const values = filterOptions(held, null, () => "x", groups).assignees.map((one) => one.value);
    expect(values.filter((one) => one === "agent:agent-1")).toHaveLength(1);
  });
});

// #44. A filter holds a LIST now. `issues.list` still takes one label and one
// assignee, so several is narrowed here, over the whole list the cache holds.
describe("a filter that holds several", () => {
  const held = [
    issue({ number: 3, id: "i3", labels: ["bug"], status: "ready" }),
    issue({ number: 2, id: "i2", labels: ["ux"], status: "done" }),
    issue({ number: 1, id: "i1", labels: ["perf"], status: "ready" }),
  ];

  it("keeps an issue wearing ANY of the chosen labels", () => {
    expect(filterIssues(held, { label: ["bug", "perf"] }).map((one) => one.id)).toEqual(["i3", "i1"]);
  });

  it("reads a bare string as a selection of one, so an older caller still works", () => {
    expect(filterIssues(held, { label: "bug" }).map((one) => one.id)).toEqual(["i3"]);
  });

  it("narrows nothing on an empty selection", () => {
    expect(filterIssues(held, { label: [], assignee: [] })).toHaveLength(3);
  });

  it("keeps an issue held by any of the chosen assignees", () => {
    const mixed = [
      issue({ id: "u", assignee: { kind: "user" } }),
      issue({ id: "a", assignee: { kind: "agent", agent_id: "agent-1" } }),
      issue({ id: "n", assignee: null }),
    ];
    expect(filterIssues(mixed, { assignee: ["user", "none"] }).map((one) => one.id)).toEqual(["u", "n"]);
    expect(filterIssues(mixed, { assignee: ["any"] }).map((one) => one.id)).toEqual(["u", "a"]);
  });

  // The param goes out only when exactly one thing is chosen: two chosen is
  // not a narrower read, it is a wider one narrowed here afterwards.
  it("sends the param for one chosen and none for several", () => {
    expect(issueListParams("proj-1", { label: ["bug"] })).toEqual({ project_id: "proj-1", label: "bug" });
    expect(issueListParams("proj-1", { label: ["bug", "ux"] })).toEqual({ project_id: "proj-1" });
    expect(issueListParams("proj-1", { assignee: ["user"] })).toEqual({ project_id: "proj-1", assignee: { kind: "user" } });
    expect(issueListParams("proj-1", { assignee: ["user", "none"] })).toEqual({ project_id: "proj-1" });
  });

  it("still calls a read narrowed when several are chosen", () => {
    expect(narrowsTheRead({ label: ["bug", "ux"] })).toBe(true);
    expect(narrowsTheRead({ label: [] })).toBe(false);
  });

  it("calls the bar set once a selection differs from the default", () => {
    expect(filtersAreSet({ state: ["open"] })).toBe(false);
    expect(filtersAreSet({ state: ["open"], label: ["bug"] })).toBe(true);
    expect(filtersAreSet({ state: [] })).toBe(true);
  });
});

describe("the order the list answers in", () => {
  it("sorts newest first by number, so an optimistic insert lands where the next read puts it", () => {
    expect(sortIssues([issue({ number: 2 }), issue({ number: 12 }), issue({ number: 1 })]).map((one) => one.number))
      .toEqual([12, 2, 1]);
  });

  it("answers a new array rather than sorting the caller's", () => {
    const given = [issue({ number: 1 }), issue({ number: 2 })];
    sortIssues(given);
    expect(given.map((one) => one.number)).toEqual([1, 2]);
  });
});

// #33. Zech, deciding the open/closed question #28 raised: "If closed means
// done we shouldn't show them in the default view."
describe("what the tab opens on", () => {
  it("is Open, not everything", () => {
    expect(DEFAULT_FILTERS.state).toBe("open");
    expect(DEFAULT_FILTERS).toEqual({ state: "open", status: "", assignee: "", label: "" });
  });

  // NO_FILTERS is still "nothing narrowed" — it is what the WHOLE list is read
  // with, which is what the filter menus are built from. A menu offering only
  // the labels of open issues could not offer Closed at all.
  it("leaves the unnarrowed read unnarrowed", () => {
    expect(NO_FILTERS.state).toBe("");
  });

  it("asks the bridge for open issues by default", () => {
    expect(issueListParams("proj-1", DEFAULT_FILTERS)).toEqual({ project_id: "proj-1", state: "open" });
  });

  // "Narrowed" has to mean "narrowed beyond the default", or the Clear press
  // would never go away and an empty project would read as an empty filter.
  it("does not call the default a narrowing", () => {
    expect(filtersAreSet(DEFAULT_FILTERS)).toBe(false);
    expect(filtersAreSet({ ...DEFAULT_FILTERS, state: "" })).toBe(true);
    expect(filtersAreSet({ ...DEFAULT_FILTERS, state: "closed" })).toBe(true);
    expect(filtersAreSet({ ...DEFAULT_FILTERS, label: "bug" })).toBe(true);
  });

  // A tab standing on the default never asked for closed issues, so it never
  // has any to hide; one that asked for everything shows them.
  it("keeps closed issues out of the default list and lets them in when asked", () => {
    const issues = [
      issue({ number: 1, state: "open" }),
      issue({ number: 2, state: "closed" }),
    ];
    expect(filterIssues(issues, DEFAULT_FILTERS).map((one) => one.number)).toEqual([1]);
    expect(filterIssues(issues, { ...DEFAULT_FILTERS, state: "" }).map((one) => one.number)).toEqual([1, 2]);
    expect(filterIssues(issues, { ...DEFAULT_FILTERS, state: "closed" }).map((one) => one.number)).toEqual([2]);
  });
});
