// The fold-aware keyed entry: one file's html is a function of what the shape
// says about it, the fold the reader put it in, and the body that is cached.
// A collapsed file costs a header and a peek; an open one costs its diff.

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  COLLAPSED_PREVIEW_ROWS,
  fileEntry,
  fileStackEntries,
  fileViewFromParsedFile,
  fileViewFromStatus,
  openFilePaths,
} from "../src/core/fileEntries.js";
import { createFileFolds, fileKey, parseDiff } from "../src/core/diff.js";
import { hashFileRows } from "../src/core/reviewMemory.js";

const lines = (count) => Array.from({ length: count }, (_unused, index) => `+line ${index}`).join("\n");

const patchFor = (path, added) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,1 +1,${added} @@\n-old\n${lines(added)}\n`;

const statusFile = (path, overrides = {}) => ({
  path,
  staged: "none",
  index_status: "-",
  worktree_status: "M",
  content_key: "key-1",
  added: 12,
  deleted: 3,
  binary: false,
  ...overrides,
});

const bodyFor = (path, added, contentKey = "key-1") => ({
  content_key: contentKey,
  patch: patchFor(path, added),
  truncated: false,
});

const rowCount = (html) => (html.match(/<tr /g) || []).length;

describe("fileViewFromStatus", () => {
  it("carries the wire's content key, counts and path", () => {
    const view = fileViewFromStatus(statusFile("src/a.js"));
    expect(view).toEqual({ path: "src/a.js", status: "EDIT", add: 12, del: 3, contentKey: "key-1", rows: null });
  });

  it.each([
    ["A", "ADD"],
    ["?", "ADD"],
    ["D", "DEL"],
    ["M", "EDIT"],
  ])("reads the worktree status letter %s as %s", (letter, word) => {
    expect(fileViewFromStatus(statusFile("a.js", { worktree_status: letter })).status).toBe(word);
  });

  it("falls back to the index status where the worktree has none", () => {
    expect(fileViewFromStatus(statusFile("a.js", { worktree_status: "-", index_status: "A" })).status).toBe("ADD");
  });
});

describe("fileViewFromParsedFile", () => {
  it("derives its content key from the rows it was parsed from, and keeps them", () => {
    const parsed = parseDiff(patchFor("src/a.js", 3))[0];
    const view = fileViewFromParsedFile(parsed);
    expect(view.contentKey).toBe(hashFileRows(parsed));
    expect(view.rows).toBe(parsed.rows);
    expect(view.path).toBe("src/a.js");
  });
});

describe("fileEntry", () => {
  const view = () => fileViewFromStatus(statusFile("src/a.js"));

  it("is keyed the way the stack has always keyed a file", () => {
    expect(fileEntry(view(), { fold: "open" }).key).toBe(fileKey({ status: "EDIT", path: "src/a.js" }));
  });

  it("draws every row of an open file from its cached body", () => {
    const { html } = fileEntry(view(), { fold: "open", body: bodyFor("src/a.js", 20) });
    expect(rowCount(html)).toBe(23); // a hunk header, a deletion, twenty additions, the patch's last newline
    expect(html).toContain('data-new-line="20"');
  });

  it("draws a short loading body for an open file whose body has not arrived", () => {
    const { html } = fileEntry(view(), { fold: "open" });
    expect(rowCount(html)).toBe(0);
    expect(html).toContain("loading");
  });

  it("draws a collapsed file's header and only its first rows", () => {
    const { html } = fileEntry(view(), { fold: "shut", body: bodyFor("src/a.js", 20) });
    expect(rowCount(html)).toBe(COLLAPSED_PREVIEW_ROWS);
    expect(html).toContain("src/a.js");
    expect(html).toContain("+12");
  });

  it("draws a collapsed file with no body as a header over an expand affordance", () => {
    const { html } = fileEntry(view(), { fold: "shut" });
    expect(rowCount(html)).toBe(0);
    expect(html).toContain("src/a.js");
    expect(html).toContain("expand");
  });

  it("treats a body of the content the file held before its last edit as missing", () => {
    const { html } = fileEntry(view(), { fold: "open", body: bodyFor("src/a.js", 20, "key-0") });
    expect(rowCount(html)).toBe(0);
    expect(html).toContain("loading");
  });

  it("says the same thing byte for byte while the key, the fold and the body hold still", () => {
    const body = bodyFor("src/a.js", 20);
    expect(fileEntry(view(), { fold: "open", body }).html).toBe(fileEntry(view(), { fold: "open", body }).html);
  });

  it("says something different when the fold, the body or the content key moves", () => {
    const body = bodyFor("src/a.js", 20);
    const open = fileEntry(view(), { fold: "open", body }).html;
    expect(fileEntry(view(), { fold: "shut", body }).html).not.toBe(open);
    expect(fileEntry(view(), { fold: "open" }).html).not.toBe(open);
    const moved = fileViewFromStatus(statusFile("src/a.js", { content_key: "key-2" }));
    expect(fileEntry(moved, { fold: "open", body: bodyFor("src/a.js", 21, "key-2") }).html).not.toBe(open);
  });

  it("carries the changed-since-your-review chip on a collapsed file too", () => {
    const { html } = fileEntry(view(), { fold: "shut", changedSince: new Set(["src/a.js"]) });
    expect(html).toContain("changed since your review");
  });

  it("renders a git.show file from the rows the payload carried", () => {
    const parsed = parseDiff(patchFor("src/b.js", 4))[0];
    const { html } = fileEntry(fileViewFromParsedFile(parsed), { fold: "open" });
    expect(rowCount(html)).toBe(7);
  });
});

// The peek is markup only if the sheet lets it through: the collapse rule hides
// the full body, and the box the peek sits in must not be that box.
const stylesSource = readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** What a rule body settles on for one property, the way the cascade does. */
const declarationOf = (body, property) =>
  body
    .split(";")
    .map((piece) => piece.split(":"))
    .filter((piece) => piece.length === 2 && piece[0].trim() === property)
    .map((piece) => piece[1].trim())
    .pop() || null;

const collapsedSelectorsWhere = (property, value) =>
  [...stylesSource.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)]
    .map((rule) => ({ selector: rule[1].trim().replace(/\s+/g, " "), body: rule[2] }))
    .filter((rule) => rule.selector.startsWith(".file.collapsed ") && declarationOf(rule.body, property) === value)
    .map((rule) => rule.selector);

/** The class of the box a file's body — its table, or the line that stands in
 *  for one — is drawn in. */
const bodyBoxOf = (html) => html.match(/<div class="([^"]+)">(?:<table>|<div class="dload">)/)[1];

describe("the collapsed peek", () => {
  const view = () => fileViewFromStatus(statusFile("src/a.js"));

  it.each([
    ["its cached rows", bodyFor("src/a.js", 20)],
    ["its expand affordance", undefined],
  ])("draws %s in a box the collapse rule leaves on screen", (_unused, body) => {
    const box = bodyBoxOf(fileEntry(view(), { fold: "shut", body }).html);
    expect(collapsedSelectorsWhere("display", "none")).not.toContain(`.file.collapsed .${box}`);
    expect(collapsedSelectorsWhere("display", "block")).toContain(`.file.collapsed .${box}`);
  });

  it("leaves the full body in the box the collapse rule hides", () => {
    const box = bodyBoxOf(fileEntry(view(), { fold: "open", body: bodyFor("src/a.js", 20) }).html);
    expect(collapsedSelectorsWhere("display", "none")).toContain(`.file.collapsed .${box}`);
  });
});

describe("fileStackEntries", () => {
  const views = () => [statusFile("src/a.js"), statusFile("uv.lock")].map(fileViewFromStatus);
  const bodies = { "src/a.js": bodyFor("src/a.js", 20), "uv.lock": bodyFor("uv.lock", 2) };
  const bodyOf = (path) => bodies[path];

  it("gives the keyed list one entry per file, in shape order", () => {
    const entries = fileStackEntries(views(), { bodyOf });
    expect(entries.map((entry) => entry.key)).toEqual(["EDIT:src/a.js", "noise"]);
  });

  it("draws the bodies the cache holds and nothing for the ones it does not", () => {
    const [first] = fileStackEntries(views(), { bodyOf: (path) => (path === "src/a.js" ? bodies[path] : undefined) });
    expect(rowCount(first.html)).toBe(23);
  });

  it("draws only a peek for a file the reader folded shut", () => {
    const folds = createFileFolds();
    folds.press("EDIT:src/a.js"); // capped → shut
    const [first] = fileStackEntries(views(), { bodyOf, folds });
    expect(rowCount(first.html)).toBe(COLLAPSED_PREVIEW_ROWS);
  });

  it("says the stack is empty when the shape names no file", () => {
    expect(fileStackEntries([], { bodyOf, empty: "No uncommitted changes." })).toEqual([
      { key: "empty", html: '<div class="empty">No uncommitted changes.</div>' },
    ]);
  });
});

describe("openFilePaths", () => {
  it("names every file whose body is on screen, and no file folded shut", () => {
    const views = [statusFile("src/a.js"), statusFile("src/b.js")].map(fileViewFromStatus);
    const folds = createFileFolds();
    folds.press("EDIT:src/b.js");
    expect([...openFilePaths(views, { folds })]).toEqual(["src/a.js"]);
  });

  it("counts a file the reader has ticked off as read as folded shut", () => {
    const views = [statusFile("src/a.js")].map(fileViewFromStatus);
    expect([...openFilePaths(views, { viewed: new Set(["src/a.js"]) })]).toEqual([]);
  });
});
