import { describe, it, expect } from "vitest";
import {
  isNoiseFile,
  groupNoiseFiles,
  noiseGroupLabel,
  uncommittedTotals,
  hasUncommittedChanges,
  defaultChangesSelection,
  selectionAfterPoll,
  commitBoxVisible,
  commitAllPaths,
  commentsSupported,
  commentLayerBusy,
} from "../src/core/changesModel.js";

const status = (overrides = {}) => ({
  branch: "main",
  head: "f".repeat(40),
  files: [{ path: "a.js", staged: "none", index_status: "M", worktree_status: "M" }],
  stat: { files_changed: 1, insertions: 4, deletions: 2 },
  patch: "diff --git a/a.js b/a.js\n",
  ...overrides,
});

const log = (overrides = {}) => ({
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "s", author: "z", time: 1 }],
  more: false,
  ...overrides,
});

describe("isNoiseFile", () => {
  it("names Build metadata, python caches, and lockfiles as noise", () => {
    expect(isNoiseFile(".build/state.json")).toBe(true);
    expect(isNoiseFile("src/__pycache__/mod.pyc")).toBe(true);
    expect(isNoiseFile("uv.lock")).toBe(true);
    expect(isNoiseFile("web/package-lock.json")).toBe(true);
    expect(isNoiseFile("Cargo.lock")).toBe(true);
    expect(isNoiseFile("go.sum")).toBe(true);
  });

  it("leaves source files alone (a path that merely mentions a lockfile is source)", () => {
    expect(isNoiseFile("src/main.py")).toBe(false);
    expect(isNoiseFile("src/build/main.py")).toBe(false);
    expect(isNoiseFile("src/lockfile_reader.py")).toBe(false);
    expect(isNoiseFile("")).toBe(false);
    expect(isNoiseFile(undefined)).toBe(false);
  });
});

describe("groupNoiseFiles", () => {
  const files = [
    { path: ".build/state.json" },
    { path: "src/main.py" },
    { path: "uv.lock" },
    { path: "src/__pycache__/mod.pyc" },
  ];

  it("keeps every file — noise is grouped, never dropped", () => {
    const grouped = groupNoiseFiles(files);
    expect(grouped.files.map((f) => f.path)).toEqual(["src/main.py"]);
    expect(grouped.noise.map((f) => f.path)).toEqual([".build/state.json", "uv.lock", "src/__pycache__/mod.pyc"]);
    expect(grouped.files.length + grouped.noise.length).toBe(files.length);
  });

  it("preserves the incoming order within each group and never mutates the input", () => {
    const input = [...files];
    groupNoiseFiles(input);
    expect(input.map((f) => f.path)).toEqual(files.map((f) => f.path));
  });

  it("tolerates empty and missing lists", () => {
    expect(groupNoiseFiles([])).toEqual({ files: [], noise: [] });
    expect(groupNoiseFiles(null)).toEqual({ files: [], noise: [] });
  });
});

describe("noiseGroupLabel", () => {
  it("counts the collapsed files, singular and plural", () => {
    expect(noiseGroupLabel(1)).toBe("1 generated file — lockfiles, caches, Build metadata");
    expect(noiseGroupLabel(4)).toBe("4 generated files — lockfiles, caches, Build metadata");
  });
});

describe("uncommittedTotals", () => {
  it("reports insertions and deletions, not a file count", () => {
    expect(uncommittedTotals(status())).toEqual({ insertions: 4, deletions: 2, files: 1 });
  });

  it("reads zeroes from an absent stat", () => {
    expect(uncommittedTotals({ files: [] })).toEqual({ insertions: 0, deletions: 0, files: 0 });
    expect(uncommittedTotals(null)).toEqual({ insertions: 0, deletions: 0, files: 0 });
  });
});

describe("hasUncommittedChanges", () => {
  it("is true only while the tree carries files", () => {
    expect(hasUncommittedChanges(status())).toBe(true);
    expect(hasUncommittedChanges(status({ files: [] }))).toBe(false);
    expect(hasUncommittedChanges(null)).toBe(false);
  });
});

describe("defaultChangesSelection", () => {
  it("opens on the review aggregate wherever the surface has one", () => {
    expect(defaultChangesSelection({ status: status(), log: log(), review: true })).toBe("review");
    expect(defaultChangesSelection({ status: status({ files: [] }), log: log(), review: true })).toBe("review");
  });

  it("opens on Uncommitted while the tree is dirty and there is no aggregate", () => {
    expect(defaultChangesSelection({ status: status(), log: log() })).toBe("uncommitted");
  });

  it("opens a clean branch at the commit list — nothing selected, no commit box", () => {
    expect(defaultChangesSelection({ status: status({ files: [] }), log: log() })).toBeNull();
  });
});

describe("selectionAfterPoll", () => {
  it("keeps whatever the user selected", () => {
    expect(selectionAfterPoll("uncommitted", status(), { review: true })).toBe("uncommitted");
    expect(selectionAfterPoll("a".repeat(40), status())).toBe("a".repeat(40));
    expect(selectionAfterPoll("uncommitted", status({ files: [] }))).toBe("uncommitted");
  });

  it("moves an empty selection onto the aggregate where there is one", () => {
    expect(selectionAfterPoll(null, status({ files: [] }), { review: true })).toBe("review");
  });

  it("moves an empty selection onto Uncommitted once the tree goes dirty", () => {
    expect(selectionAfterPoll(null, status())).toBe("uncommitted");
    expect(selectionAfterPoll(null, status({ files: [] }))).toBeNull();
  });
});

describe("commitBoxVisible", () => {
  it("discloses the commit box only while uncommitted changes exist", () => {
    expect(commitBoxVisible(status())).toBe(true);
    expect(commitBoxVisible(status({ files: [] }))).toBe(false);
  });
});

describe("commitAllPaths", () => {
  it("names every changed path — commit is commit-all, there is no staged set", () => {
    const paths = commitAllPaths(
      status({
        files: [
          { path: "a.js", staged: "none" },
          { path: "b.js", staged: "full" },
          { path: "uv.lock", staged: "none" },
        ],
      }),
    );
    expect(paths).toEqual(["a.js", "b.js", "uv.lock"]);
  });

  it("is empty for a clean tree", () => {
    expect(commitAllPaths(status({ files: [] }))).toEqual([]);
    expect(commitAllPaths(null)).toEqual([]);
  });
});

describe("commentsSupported", () => {
  it("offers commenting where there is a run agent to send the notes to", () => {
    expect(commentsSupported({ run_id: "r1" })).toBe(true);
  });

  it("stays off for a bare project or worktree scope (nobody to send to)", () => {
    expect(commentsSupported({ project_id: "p1" })).toBe(false);
    expect(commentsSupported({ project_id: "p1", worktree_id: "w1" })).toBe(false);
    expect(commentsSupported(null)).toBe(false);
  });
});

describe("commentLayerBusy", () => {
  it("freezes the poll while the reviewer is mid-comment", () => {
    expect(commentLayerBusy({ pending: 1, popOpen: false, generalText: "", menuOpen: false })).toBe(true);
    expect(commentLayerBusy({ pending: 0, popOpen: true, generalText: "", menuOpen: false })).toBe(true);
    expect(commentLayerBusy({ pending: 0, popOpen: false, generalText: "half a thought", menuOpen: false })).toBe(true);
    expect(commentLayerBusy({ pending: 0, popOpen: false, generalText: "", menuOpen: true })).toBe(true);
    // The earliest state of all: a range still being dragged, before the
    // popover that would turn it into a comment has opened.
    expect(commentLayerBusy({ selecting: true })).toBe(true);
  });

  it("lets the poll repaint when nothing is pending", () => {
    expect(commentLayerBusy({ pending: 0, popOpen: false, generalText: "  ", menuOpen: false })).toBe(false);
    expect(commentLayerBusy({})).toBe(false);
  });
});
