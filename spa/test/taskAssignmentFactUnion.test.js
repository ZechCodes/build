import { describe, expect, it } from "vitest";
import { selectTaskAssignmentFacts } from "../src/core/taskAssignmentFacts.js";

const task = (version, blocked) => ({
  id: "task", updated_at: `2026-10-13T01:0${version}:00Z`,
  dependencies: { blocked, waiting_on: blocked ? [{ task_id: "prerequisite", resolved: false }] : [] },
});
const record = (row) => ({ task_id: row.id, updated_at: row.updated_at, dependencies: row.dependencies });
const decide = (knowledge, replicas, opened = null) => selectTaskAssignmentFacts({ knowledge, replicas, opened });

describe("dependency fact union", () => {
  it.each([false, true])("a newer replica blocker beats ready record regardless of order (%s)", (reverse) => {
    const replicas = [task(4, false), task(6, true)];
    expect(decide(record(task(5, false)), reverse ? replicas.reverse() : replicas).dependencies.blocked).toBe(true);
  });

  it("a newer explicit ready replica clears an older blocked record and opened task", () => {
    expect(decide(record(task(6, true)), [task(7, false)], task(6, true))).toEqual(task(7, false));
  });

  it.each([false, true])("a tie keeps waiting whichever source holds the blocker (%s)", (blockedRecord) => {
    expect(decide(record(task(6, blockedRecord)), [task(6, !blockedRecord)]).dependencies.blocked).toBe(true);
  });

  it("applies the conservative absence rule when no capable facts exist", () => {
    const legacy = { ...task(7, false), dependencies: undefined };
    expect(decide(null, [legacy, { ...legacy, dependencies: null }, { ...legacy, dependencies: {} }], legacy)).toBeNull();
  });

  it("legacy facts cannot veto a strictly newer capable clear", () => {
    expect(decide(record(task(5, true)), [task(7, false), { ...task(8, false), dependencies: null }]).dependencies.blocked).toBe(false);
  });

  it.each([undefined, null, {}, { blocked: false }, { blocked: false, waiting_on: null }])("unknown summary %j never proves ready", (dependencies) => {
    const unknown = { ...task(7, false), dependencies };
    expect(decide(null, [unknown])).toBeNull();
    expect(decide(record(task(6, true)), [unknown]).dependencies.blocked).toBe(true);
  });

  it("the opened picker task participates even when every cached source is older", () => {
    expect(decide(record(task(5, false)), [task(4, false)], task(6, true)).dependencies.blocked).toBe(true);
  });

  it("unknown task versions cannot erase waiting evidence", () => {
    expect(decide(record(task(6, true)), [{ ...task(7, false), updated_at: undefined }]).dependencies.blocked).toBe(true);
    expect(decide(record(task(7, false)), [{ ...task(6, true), updated_at: "invalid" }]).dependencies.blocked).toBe(true);
  });
});
