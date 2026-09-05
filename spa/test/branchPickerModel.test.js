// The create modal's Branch tab is a picker over every branch the project has.
// This is its pure half: which rows the typed text leaves standing, what each
// row promises, which call pressing it makes, and where it lands.

import { describe, it, expect } from "vitest";
import { branchPickerRows, cutNewRow, branchStartRoute, INTENT_VERB } from "../src/core/branchPickerModel.js";

const branch = (name, stamps = {}) => ({
  name,
  is_current: false,
  remote: null,
  upstream: null,
  ahead: 0,
  behind: 0,
  head_subject: "",
  head_time: 0,
  run_id: null,
  external_worktree_id: null,
  primary_worktree_id: null,
  ...stamps,
});

const rowsFor = (branches, query = "") => branchPickerRows({ projectId: "p1", branches, query });
const named = (rows, name) => rows.find((row) => row.name === name);

describe("the branch picker's rows", () => {
  it("offers a checkout of a branch nothing holds", () => {
    const [row] = rowsFor([branch("feature-x")]);
    expect(row).toMatchObject({
      key: "branch:feature-x",
      name: "feature-x",
      intent: "checkout",
      verb: INTENT_VERB.checkout,
      remote: null,
      detail: "",
      branch: "feature-x",
      focusComposer: true,
      call: { method: "worktree.create", params: { project_id: "p1", branch: "feature-x" } },
    });
  });

  it("marks a branch only a remote has, and still makes the one checkout call — the bridge fetches it", () => {
    const [row] = rowsFor([branch("feature-x", { remote: "origin" })]);
    expect(row).toMatchObject({
      intent: "materialise",
      verb: INTENT_VERB.materialise,
      remote: "origin",
      focusComposer: true,
      call: { method: "worktree.create", params: { project_id: "p1", branch: "feature-x" } },
    });
  });

  it("opens a branch a run already owns, calling nothing", () => {
    const [row] = rowsFor([branch("feature-x", { run_id: "r-1" })]);
    expect(row).toMatchObject({ intent: "open", verb: INTENT_VERB.open, call: null, branch: "feature-x", focusComposer: false });
    expect(row.detail).toContain("run");
  });

  it("adopts the worktree a branch is checked out in, naming the worktree", () => {
    const [row] = rowsFor([branch("feature-x", { external_worktree_id: "wt-3" })]);
    expect(row).toMatchObject({
      intent: "adopt",
      verb: INTENT_VERB.adopt,
      focusComposer: false,
      call: { method: "run.adopt", params: { project_id: "p1", worktree_id: "wt-3" } },
    });
    expect(row.detail).toContain("worktree");
  });

  it("adopts the primary checkout by what it is, not by an id", () => {
    const [row] = rowsFor([branch("main", { primary_worktree_id: "wt-root", is_current: true })]);
    expect(row).toMatchObject({
      intent: "adopt",
      verb: INTENT_VERB.adopt,
      call: { method: "run.adopt", params: { project_id: "p1", primary: true } },
    });
    expect(row.detail).toContain("primary");
  });

  it("lets a run speak for a branch the primary checkout is also on", () => {
    const [row] = rowsFor([branch("main", { run_id: "r-2", primary_worktree_id: "wt-root" })]);
    expect(row.intent).toBe("open");
    expect(row.call).toBeNull();
  });

  it("keeps the listing's order when nothing is typed, and ranks it when something is", () => {
    const branches = [branch("main"), branch("feature-core-moderation"), branch("fix/csv")];
    expect(rowsFor(branches).map((row) => row.name)).toEqual(["main", "feature-core-moderation", "fix/csv"]);
    expect(rowsFor(branches, "fcm").map((row) => row.name)).toEqual(["build/fcm", "feature-core-moderation"]);
  });

  it("puts a Create row first whenever the typed text names no branch of its own", () => {
    const rows = rowsFor([branch("feature-x")], "mascot model spike");
    expect(rows[0]).toMatchObject({
      key: "create-new",
      name: "build/mascot-model-spike",
      intent: "cut",
      verb: INTENT_VERB.cut,
      branch: null,
      focusComposer: true,
      call: { method: "worktree.create", params: { project_id: "p1", name: "mascot model spike" } },
    });
    expect(rows).toHaveLength(1);
  });

  it("withholds the Create row when there is nothing to cut or the branch already exists", () => {
    expect(rowsFor([branch("feature-x")], "").some((row) => row.intent === "cut")).toBe(false);
    expect(rowsFor([branch("feature-x")], "feature-x").map((row) => row.intent)).toEqual(["checkout"]);
    expect(rowsFor([branch("feature-x")], "!!!").some((row) => row.intent === "cut")).toBe(false);
  });

  it("survives a listing it was never given", () => {
    expect(branchPickerRows({ projectId: "p1" })).toEqual([]);
    expect(rowsFor([branch("a"), branch("b")], "  ").map((row) => row.name)).toEqual(["a", "b"]);
  });

  it("cuts the typed text on its own, for the caller with nothing highlighted", () => {
    expect(cutNewRow("p1", "Mascot Model Spike!")).toMatchObject({
      intent: "cut",
      name: "build/mascot-model-spike",
      branch: null,
      call: { method: "worktree.create", params: { project_id: "p1", name: "Mascot Model Spike!" } },
    });
  });
});

describe("where a pick lands", () => {
  it("opens the branch the row named", () => {
    const row = named(rowsFor([branch("feature-x", { external_worktree_id: "wt-3" })]), "feature-x");
    expect(branchStartRoute("p1", row, { run_id: "r-9" })).toEqual({ name: "branch", projectId: "p1", branch: "feature-x", tab: "changes" });
  });

  it("opens the branch the answer named, for the row that had none until the cut", () => {
    expect(branchStartRoute("p1", cutNewRow("p1", "spike"), { branch: "build/spike" })).toEqual({
      name: "branch",
      projectId: "p1",
      branch: "build/spike",
      tab: "changes",
    });
  });
});
