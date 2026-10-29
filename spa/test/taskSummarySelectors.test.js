import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { canonicalSummaryFilters, summaryFiltersSupported } from "../src/core/taskSummaryQuery.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";

const fixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/tasks.list.json", import.meta.url), "utf8")).exact_queries;
const selectors = {
  watched: false, attention_rule: "asked", linked_workspace_id: " Workspace\u0000É ", tracker_agent_id: "AgentA",
  exclude_review_mode: "snapshot", order: "anchor", done_since: 0, unfinished: false, blocked: false,
  held: false, undelivered: false, exclude_assignee: { agent_id: "AgentA", kind: "agent" },
};
const taskCaps = (...names) => capabilitiesOf({ api_version: "3.27.0", capabilities: names }).tasks;

describe("canonical exact summary selectors", () => {
  it("matches shared selectors and preserves false/zero/exact IDs with deterministic actor keys", () => {
    expect(Object.keys(selectors).sort()).toEqual(Object.keys(fixture.selectors).sort());
    expect(canonicalSummaryFilters(selectors)).toEqual(selectors);
    expect(Object.keys(canonicalSummaryFilters(selectors).exclude_assignee)).toEqual(["kind", "agent_id"]);
  });
  it("gives backend-equivalent default number ordering the same identity", () => {
    expect(canonicalSummaryFilters({ order: "number" })).toEqual(canonicalSummaryFilters({}));
    for (const order of fixture.selectors.order.filter((value) => value !== "number")) expect(canonicalSummaryFilters({ order })).toEqual({ order });
  });
  it("preserves existing dimensions, aliases, and opaque actor IDs", () => {
    expect(canonicalSummaryFilters({ states: [], labels: ["API", "api", "É"], statuses: ["In review", "in_review"],
      assignees: ["none", { agent_id: "AgentA", kind: "agent" }] })).toEqual({
      labels: ["api", "É"], statuses: ["in_review"], assignees: ["none", { kind: "agent", agent_id: "AgentA" }],
    });
    expect(() => canonicalSummaryFilters({ state: "open", states: ["open"] })).toThrow();
    expect(() => canonicalSummaryFilters({ unknown_selector: true })).toThrow();
  });
  for (const name of Object.keys(selectors)) {
    it(`rejects null and undefined for ${name}`, () => {
      for (const value of [null, undefined]) expect(() => canonicalSummaryFilters({ [name]: value })).toThrow();
    });
  }
  for (const name of ["watched", "unfinished", "blocked", "held", "undelivered"]) {
    it(`requires strict booleans for ${name}`, () => {
      expect(canonicalSummaryFilters({ [name]: true })).toEqual({ [name]: true });
      for (const value of [0, 1, "false", [], {}]) expect(() => canonicalSummaryFilters({ [name]: value })).toThrow();
    });
  }
  for (const name of ["attention_rule", "exclude_review_mode", "order"]) {
    it(`requires declared ${name} enums without normalization`, () => {
      for (const value of fixture.selectors[name]) expect(() => canonicalSummaryFilters({ [name]: value })).not.toThrow();
      for (const value of ["", "UNKNOWN", " number ", true, 1]) expect(() => canonicalSummaryFilters({ [name]: value })).toThrow();
    });
  }
  it("accepts only safe integer cutoffs, including negative and zero", () => {
    for (const done_since of [0, -1, Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER]) expect(canonicalSummaryFilters({ done_since })).toEqual({ done_since });
    for (const done_since of [1.1, Infinity, NaN, "0", Number.MAX_SAFE_INTEGER + 1, Number.MIN_SAFE_INTEGER - 1]) expect(() => canonicalSummaryFilters({ done_since })).toThrow();
  });
  for (const name of ["linked_workspace_id", "tracker_agent_id"]) {
    it(`preserves exact ${name} with raw UTF-8/NUL byte boundaries`, () => {
      for (const value of ["é".repeat(64), "\u0000".repeat(128), " A ", "a", "A"]) expect(canonicalSummaryFilters({ [name]: value })).toEqual({ [name]: value });
      for (const value of ["", "a".repeat(129), "é".repeat(64) + "a", "\u0000".repeat(129), 1, {}]) expect(() => canonicalSummaryFilters({ [name]: value })).toThrow();
    });
  }
  it("requires an exact tagged exclusion and bounded agent ID", () => {
    for (const exclude_assignee of [{ kind: "user" }, { kind: "project_agent" }, { kind: "agent", agent_id: "é".repeat(64) }, { kind: "agent", agent_id: "\u0000".repeat(128) }]) expect(canonicalSummaryFilters({ exclude_assignee })).toEqual({ exclude_assignee });
    for (const exclude_assignee of ["any", "none", [], {}, { kind: "other" }, { kind: "agent" }, { kind: "user", agent_id: "A" }, { kind: "user", extra: true }, { kind: "agent", agent_id: "" }, { kind: "agent", agent_id: 1 }, { kind: "agent", agent_id: "é".repeat(64) + "a" }, { kind: "agent", agent_id: "A", extra: true }]) expect(() => canonicalSummaryFilters({ exclude_assignee })).toThrow();
  });
});

describe("independent live selector capabilities", () => {
  it("requires summary support for unrestricted and old-dimension queries", () => {
    for (const caps of [undefined, {}, { listSummaries: 1 }]) expect(summaryFiltersSupported({}, caps)).toBe(false);
    expect(summaryFiltersSupported({ state: "open" }, taskCaps("tasks.listSummaries"))).toBe(true);
  });
  for (const [name, capability] of Object.entries(fixture.capabilities)) {
    it(`requires summaries and independent ${capability} for ${name}`, () => {
      const filters = { [name]: selectors[name] };
      expect(summaryFiltersSupported(filters, taskCaps("tasks.listSummaries"))).toBe(false);
      expect(summaryFiltersSupported(filters, taskCaps(capability))).toBe(false);
      expect(summaryFiltersSupported(filters, taskCaps("tasks.listSummaries", capability))).toBe(true);
      expect(summaryFiltersSupported(filters, taskCaps("tasks.listSummaries", "tasks.counts", "tasks.lookup"))).toBe(false);
    });
  }
  it("requires order for an explicit default and every capability in a combination", () => {
    expect(summaryFiltersSupported({ order: "number" }, taskCaps("tasks.listSummaries"))).toBe(false);
    expect(summaryFiltersSupported({ order: "number" }, taskCaps("tasks.listSummaries", "tasks.listOrder"))).toBe(true);
    const names = ["tasks.listSummaries", ...new Set(Object.values(fixture.capabilities))];
    expect(summaryFiltersSupported(selectors, taskCaps(...names))).toBe(true);
    for (const omitted of names) expect(summaryFiltersSupported(selectors, taskCaps(...names.filter((name) => name !== omitted)))).toBe(false);
  });
});
