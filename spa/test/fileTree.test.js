// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import {
  ancestorsOf,
  fileTreeHtml,
  treeKeyMove,
  visibleTreeRows,
} from "../src/core/fileTree.js";

const listings = (map) => new Map(Object.entries(map));

const TREE = listings({
  "": { path: "", entries: [{ name: "src", kind: "dir" }, { name: "link", kind: "symlink" }, { name: "README.md", kind: "file", size: 12 }] },
  src: { path: "src", entries: [{ name: "core", kind: "dir" }, { name: "a.js", kind: "file", size: 3 }] },
  "src/core": { path: "src/core", entries: [{ name: "b.js", kind: "file", size: 4 }] },
});

const shape = (rows) => rows.map((row) => `${"  ".repeat(row.depth)}${row.kind}:${row.path}`);

describe("visibleTreeRows", () => {
  it("lists the checkout's root with nothing expanded", () => {
    expect(shape(visibleTreeRows(TREE, new Set()))).toEqual(["dir:src", "symlink:link", "file:README.md"]);
  });

  it("nests an expanded directory's entries under it, one level deeper per directory", () => {
    const rows = visibleTreeRows(TREE, new Set(["src", "src/core"]));
    expect(shape(rows)).toEqual([
      "dir:src",
      "  dir:src/core",
      "    file:src/core/b.js",
      "  file:src/a.js",
      "symlink:link",
      "file:README.md",
    ]);
    expect(rows[0].expanded).toBe(true);
  });

  it("hides the children of a collapsed directory even when a deeper one is still expanded", () => {
    expect(shape(visibleTreeRows(TREE, new Set(["src/core"])))).toEqual(["dir:src", "symlink:link", "file:README.md"]);
  });

  it("shows an expanded directory with no listing yet as expanded and childless", () => {
    const rows = visibleTreeRows(listings({ "": TREE.get("") }), new Set(["src"]));
    expect(shape(rows)).toEqual(["dir:src", "symlink:link", "file:README.md"]);
    expect(rows[0].expanded).toBe(true);
  });

  it("puts a directory that could not be listed as an error row where its children would be", () => {
    const rows = visibleTreeRows(listings({ "": TREE.get(""), src: { error: "denied" } }), new Set(["src"]));
    expect(rows[1]).toMatchObject({ kind: "error", depth: 1, message: "denied" });
  });
});

describe("fileTreeHtml", () => {
  const rows = visibleTreeRows(TREE, new Set(["src"]));

  it("draws a chevron on directories, turned by aria-expanded, and indents by depth", () => {
    const host = document.createElement("div");
    host.innerHTML = fileTreeHtml(rows, {});
    const src = host.querySelector('[data-path="src"]');
    expect(src.classList.contains("fdir")).toBe(true);
    expect(src.getAttribute("aria-expanded")).toBe("true");
    expect(src.querySelector(".fchev")).toBeTruthy();
    expect(host.querySelector('[data-path="src/core"]').getAttribute("aria-expanded")).toBe("false");
    expect(host.querySelector('[data-path="src/a.js"]').style.getPropertyValue("--depth")).toBe("1");
    expect(host.querySelector('[data-path="src/a.js"]').getAttribute("aria-level")).toBe("2");
    expect(host.querySelector('[data-path="README.md"]').hasAttribute("aria-expanded")).toBe(false);
  });

  it("has no breadcrumb and no up row: the root is the checkout", () => {
    const html = fileTreeHtml(rows, {});
    expect(html).not.toContain("fcrumb");
    expect(html).not.toContain("fup");
  });

  it("marks the open file and the keyboard selection as two different states", () => {
    const host = document.createElement("div");
    host.innerHTML = fileTreeHtml(rows, { openPath: "src/a.js", cursorPath: "README.md" });
    const open = host.querySelector('[data-path="src/a.js"]');
    const cursor = host.querySelector('[data-path="README.md"]');
    expect(open.classList.contains("sel")).toBe(true);
    expect(open.getAttribute("aria-current")).toBe("true");
    expect(open.classList.contains("cursor")).toBe(false);
    expect(cursor.classList.contains("cursor")).toBe(true);
    expect(cursor.getAttribute("aria-selected")).toBe("true");
    expect(cursor.getAttribute("tabindex")).toBe("0");
    expect(host.querySelectorAll('[tabindex="0"]')).toHaveLength(1);
  });

  it("leaves the first row reachable by Tab when nothing is selected", () => {
    const host = document.createElement("div");
    host.innerHTML = fileTreeHtml(rows, {});
    expect(host.querySelector('[tabindex="0"]').dataset.path).toBe("src");
  });

  it("highlights nothing when the open file is not in the visible tree", () => {
    const host = document.createElement("div");
    host.innerHTML = fileTreeHtml(rows, { openPath: "src/core/b.js" });
    expect(host.querySelector(".sel")).toBeNull();
  });

  // Repo file names are untrusted input (spec §9): a name is escaped in the
  // row's label AND in the data attribute the click wiring reads back.
  it("escapes hostile names in labels and in data attributes", () => {
    const hostile = listings({
      "": {
        path: "",
        entries: [
          { kind: "dir", name: '<img src=x onerror=alert(1)>" onmouseover="alert(4)' },
          { kind: "file", name: "<script>alert(2)</script>.txt", size: 3 },
          { kind: "symlink", name: "<svg onload=alert(3)>" },
        ],
      },
    });
    const html = fileTreeHtml(visibleTreeRows(hostile, new Set()), {});
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<svg");
    expect(html).not.toContain('onmouseover="alert(4)"');
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelectorAll("img, script, svg")).toHaveLength(0);
    expect(host.querySelector(".fdir").dataset.path).toBe('<img src=x onerror=alert(1)>" onmouseover="alert(4)');
    expect(host.querySelector(".ffile").dataset.path).toBe("<script>alert(2)</script>.txt");
  });

  it("escapes an error row's message", () => {
    const html = fileTreeHtml([{ kind: "error", path: "src", depth: 1, message: "<b>no</b>" }], {});
    expect(html).not.toContain("<b>");
  });

  it("gives every name a box of its own to truncate inside", () => {
    const html = fileTreeHtml(rows, {});
    expect(html.match(/class="fname"/g)).toHaveLength(rows.length);
  });

  it("says a root with no entries is empty", () => {
    expect(fileTreeHtml([], {})).toContain("Empty directory.");
  });
});

describe("treeKeyMove", () => {
  const rows = visibleTreeRows(TREE, new Set(["src"]));

  it("moves the selection down and up through the visible rows, stopping at the ends", () => {
    expect(treeKeyMove(rows, "src", "ArrowDown")).toEqual({ cursor: "src/core" });
    expect(treeKeyMove(rows, "src/core", "ArrowUp")).toEqual({ cursor: "src" });
    expect(treeKeyMove(rows, "src", "ArrowUp")).toEqual({ cursor: "src" });
    expect(treeKeyMove(rows, "README.md", "ArrowDown")).toEqual({ cursor: "README.md" });
  });

  it("starts at the first row when nothing is selected", () => {
    expect(treeKeyMove(rows, null, "ArrowDown")).toEqual({ cursor: "src" });
  });

  it("expands a collapsed directory on Right and collapses an expanded one on Left", () => {
    expect(treeKeyMove(rows, "src/core", "ArrowRight")).toEqual({ expand: "src/core" });
    expect(treeKeyMove(rows, "src", "ArrowLeft")).toEqual({ collapse: "src" });
  });

  it("steps into an expanded directory on Right and out to the parent on Left", () => {
    expect(treeKeyMove(rows, "src", "ArrowRight")).toEqual({ cursor: "src/core" });
    expect(treeKeyMove(rows, "src/a.js", "ArrowLeft")).toEqual({ cursor: "src" });
  });

  it("opens a file and toggles a directory on Enter", () => {
    expect(treeKeyMove(rows, "src/a.js", "Enter")).toEqual({ open: "src/a.js" });
    expect(treeKeyMove(rows, "src", "Enter")).toEqual({ collapse: "src" });
    expect(treeKeyMove(rows, "src/core", "Enter")).toEqual({ expand: "src/core" });
  });

  it("ignores keys it does not own", () => {
    expect(treeKeyMove(rows, "src", "a")).toBeNull();
  });
});

describe("ancestorsOf", () => {
  it("names every directory above a path, outermost first", () => {
    expect(ancestorsOf("a/b/c.txt")).toEqual(["a", "a/b"]);
    expect(ancestorsOf("c.txt")).toEqual([]);
  });
});
