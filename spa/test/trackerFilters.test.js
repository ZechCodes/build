// The Issues tab's filters. They match `issues.list`'s params one for one — a
// filter is a param, not a client-side pass over everything — and the same
// narrowing is applied locally to the cached list while the read is in flight.

import { describe, expect, it } from "vitest";
import {
  ANY_ASSIGNEE,
  NO_FILTERS,
  assigneesOf,
  filterIssues,
  filterOptions,
  filtersAreSet,
  issueListParams,
  labelsOf,
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
  it("says whether anything is narrowed", () => {
    expect(filtersAreSet(NO_FILTERS)).toBe(false);
    expect(filtersAreSet({ ...NO_FILTERS, label: "bug" })).toBe(true);
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

  it("names each assignee through the caller, which knows this project's agents", () => {
    const options = filterOptions(issues, null, (assignee) => (assignee.kind === "user" ? "You" : "Agent 1"));
    expect(options.assignees.map((one) => one.label)).toEqual([
      "Anyone", "Anyone assigned", "Unassigned", "Agent 1", "You",
    ]);
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
