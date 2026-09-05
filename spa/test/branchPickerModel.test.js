// The create modal's Branch tab is a picker over every branch the project has.
// This is its pure half: which rows the typed text leaves standing, what each
// row promises, which call pressing it makes, where it lands, and which row an
// Enter press means.

import { describe, it, expect } from "vitest";
import { branchPickerRows, pressedRow, INTENT_VERB } from "../src/core/branchPickerModel.js";

const branch = (name, stamps = {}) => ({
  name,
  is_current: false,
  remote: null,
  upstream: null,
  ahead: 0,
  behind: 0,
  head_subject: "",
  head_time: 0,
  holder: null,
  ...stamps,
});

const heldBy = (kind, id) => ({ holder: { kind, id } });

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

  it("opens a branch a run already holds, calling nothing", () => {
    const [row] = rowsFor([branch("feature-x", heldBy("run", "r-1"))]);
    expect(row).toMatchObject({ intent: "open", verb: INTENT_VERB.open, call: null, branch: "feature-x", focusComposer: false });
    expect(row.detail).toContain("run");
  });

  it("adopts the worktree a branch is checked out in, naming the worktree", () => {
    const [row] = rowsFor([branch("feature-x", heldBy("external_worktree", "wt-3"))]);
    expect(row).toMatchObject({
      intent: "adopt",
      verb: INTENT_VERB.adopt,
      focusComposer: false,
      call: { method: "run.adopt", params: { project_id: "p1", worktree_id: "wt-3" } },
    });
    expect(row.detail).toContain("worktree");
  });

  it("adopts the primary checkout by what it is, not by an id", () => {
    const [row] = rowsFor([branch("main", { is_current: true, ...heldBy("primary_checkout", "wt-root") })]);
    expect(row).toMatchObject({
      intent: "adopt",
      verb: INTENT_VERB.adopt,
      call: { method: "run.adopt", params: { project_id: "p1", primary: true } },
    });
    expect(row.detail).toContain("primary");
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
});

describe("where a pressed row lands", () => {
  it("opens the branch the row named", () => {
    const row = named(rowsFor([branch("feature-x", heldBy("external_worktree", "wt-3"))]), "feature-x");
    expect(row.land({ run_id: "r-9" })).toEqual({
      route: { name: "branch", projectId: "p1", branch: "feature-x", tab: "changes" },
      focusComposer: false,
    });
  });

  it("opens the branch the answer named, for the row that had none until the cut", () => {
    const [cut] = rowsFor([branch("feature-x")], "spike");
    expect(cut.land({ branch: "build/spike" })).toEqual({
      route: { name: "branch", projectId: "p1", branch: "build/spike", tab: "changes" },
      focusComposer: true,
    });
  });
});

describe("which row an Enter press means", () => {
  const rows = rowsFor([branch("feature-x")], "feature-x");

  it("presses the highlighted row", () => {
    expect(pressedRow({ rows, query: "feature-x", highlight: 0 })).toBe(rows[0]);
  });

  it("presses the leading row when nothing is highlighted but something is typed", () => {
    const typed = rowsFor([branch("feature-x")], "spike");
    expect(pressedRow({ rows: typed, query: "spike", highlight: -1 })).toBe(typed[0]);
    expect(pressedRow({ rows: typed, query: "spike", highlight: -1 }).intent).toBe("cut");
  });

  it("means the listed branch, not a second one beside it, when the text spells one exactly", () => {
    const pressed = pressedRow({ rows, query: "feature-x", highlight: -1 });
    expect(pressed.intent).toBe("checkout");
    expect(pressed.call).toEqual({ method: "worktree.create", params: { project_id: "p1", branch: "feature-x" } });
  });

  it("presses nothing on an empty field, or when the text leaves no row standing", () => {
    expect(pressedRow({ rows: rowsFor([branch("feature-x")]), query: "", highlight: -1 })).toBeNull();
    expect(pressedRow({ rows: rowsFor([], "!!!"), query: "!!!", highlight: -1 })).toBeNull();
    expect(pressedRow({ rows: [], query: "", highlight: -1 })).toBeNull();
  });
});
