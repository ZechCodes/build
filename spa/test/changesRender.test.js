import { describe, it, expect } from "vitest";
import {
  changesRailEntries,
  commitRowHtml,
  uncommittedHeaderHtml,
  commitHeaderHtml,
  commentTrayHtml,
  changesetPlaceholderHtml,
} from "../src/core/changesRender.js";

const NOW = 1_750_000_000;

const status = (overrides = {}) => ({
  branch: "main",
  head: "f".repeat(40),
  files: [
    { path: "src/a.js", staged: "none", index_status: "M", worktree_status: "M" },
    { path: "notes.txt", staged: "none", index_status: "?", worktree_status: "?" },
  ],
  stat: { files_changed: 2, insertions: 7, deletions: 3 },
  patch: "",
  truncated: false,
  ...overrides,
});

const commit = (overrides = {}) => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "fix the widget",
  author: "Ada",
  email: "z@example.com",
  time: NOW - 120,
  ...overrides,
});

const log = (overrides = {}) => ({
  commits: [commit(), commit({ hash: "c".repeat(40), short: "ccccccc", subject: "older" })],
  more: false,
  ...overrides,
});

const rail = (overrides = {}) =>
  changesRailEntries({ status: status(), log: log(), selected: "uncommitted", nowSeconds: NOW, ...overrides })
    .map((entry) => entry.html)
    .join("");

describe("changesRailEntries", () => {
  it("puts Uncommitted at the top, above the commit list", () => {
    const html = rail();
    expect(html.indexOf('data-sel="uncommitted"')).toBeGreaterThan(-1);
    expect(html.indexOf('data-sel="uncommitted"')).toBeLessThan(html.indexOf('data-hash='));
  });

  it("puts the review aggregate above Uncommitted where the surface has one", () => {
    const html = rail({ review: { base: "main" } });
    expect(html.indexOf('data-sel="review"')).toBeLessThan(html.indexOf('data-sel="uncommitted"'));
  });

  it("carries +/− counts on Uncommitted, not a file count", () => {
    const html = rail();
    expect(html).toContain("+7");
    expect(html).toContain("−3");
    expect(html).not.toContain("2 files");
  });

  it("says clean when there is nothing uncommitted", () => {
    expect(rail({ status: status({ files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } }) })).toContain("clean");
  });

  it("renders a row per commit under a Commits head", () => {
    const html = rail();
    expect(html).toContain(`data-hash="${"a".repeat(40)}"`);
    expect(html).toContain(`data-hash="${"c".repeat(40)}"`);
    expect(html).toContain("Commits");
  });

  it("offers the review aggregate above the commit list, only when the surface has one", () => {
    expect(rail()).not.toContain('data-sel="review"');
    const html = rail({ review: { base: "main" } });
    expect(html).toContain('data-sel="review"');
    expect(html).toContain("All changes");
    expect(html).toContain("vs main");
    expect(html.indexOf('data-sel="review"')).toBeLessThan(html.indexOf('data-hash='));
  });

  it("marks exactly the selected entry", () => {
    const html = rail({ selected: "a".repeat(40) });
    expect(html).toContain('crow sel"');
    expect(html.match(/rrow sel/g)).toBeNull();
  });

  it("selects nothing when the selection is null (a clean branch's commit list)", () => {
    const html = rail({ selected: null, status: status({ files: [] }) });
    expect(html).not.toContain("sel\"");
  });

  it("offers the older-commits affordance only when another page exists", () => {
    expect(rail({ log: log({ more: true }) })).toContain('class="gitmore"');
    expect(rail()).not.toContain("gitmore");
  });

  it("renders the empty state for a repo with no commits", () => {
    expect(rail({ log: log({ commits: [] }) })).toContain("No commits yet.");
  });

  it("escapes the review base branch", () => {
    expect(rail({ review: { base: "<img src=x>" } })).not.toContain("<img");
  });
});

describe("commitRowHtml", () => {
  it("renders short hash, subject, author, and relative time", () => {
    const html = commitRowHtml(commit(), { nowSeconds: NOW });
    expect(html).toContain(`data-hash="${"a".repeat(40)}"`);
    expect(html).toContain("fix the widget");
    expect(html).toContain("2m ago");
  });

  it("escapes a malicious subject and author", () => {
    const html = commitRowHtml(commit({ subject: "<img src=x onerror=alert(1)>", author: "<b>evil</b>" }), { nowSeconds: NOW });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>evil</b>");
  });

  it("marks commits ahead of the base branch and the selected row", () => {
    expect(commitRowHtml(commit({ ahead_of_base: true }), { nowSeconds: NOW })).toContain("ahead");
    expect(commitRowHtml(commit(), { selected: true, nowSeconds: NOW })).toContain("sel");
  });

  it("marks commits included in All changes as not pushed", () => {
    const html = commitRowHtml(commit({ unpushed: true }), { nowSeconds: NOW });
    expect(html).toContain('class="crow unpushed"');
    expect(html).toContain('title="Not pushed"');
    expect(commitRowHtml(commit({ unpushed: false }), { nowSeconds: NOW })).not.toContain("Not pushed");
  });
});

describe("uncommittedHeaderHtml", () => {
  it("shows the changeset counts without repeating its rail label", () => {
    const html = uncommittedHeaderHtml(status());
    expect(html).not.toContain("Uncommitted changes");
    expect(html).toContain("2 files");
    expect(html).toContain("+7");
    expect(html).toContain("−3");
  });

  it("surfaces the file-list truncation notice when the bridge capped the shape", () => {
    expect(uncommittedHeaderHtml(status({ files_truncated: true }))).toContain("file list truncated");
    expect(uncommittedHeaderHtml(status())).not.toContain("truncated");
  });

  // git.status ships shape and no patch at all, so there is no whole-changeset
  // diff for a 1 MiB cap to fall on. The cap falls on one file's body, and the
  // file is where it is drawn (core/fileEntries.js).
  it("says nothing about a capped diff over the whole changeset", () => {
    expect(uncommittedHeaderHtml(status({ truncated: true }))).not.toContain("diff truncated");
  });
});

describe("commitHeaderHtml", () => {
  const show = (overrides = {}) => ({
    hash: "b".repeat(40),
    short: "bbbbbbb",
    subject: "add feature",
    body: "long explanation",
    author: "Ada",
    email: "z@example.com",
    stat: { files_changed: 1, insertions: 3, deletions: 1 },
    truncated: false,
    ...overrides,
  });

  it("renders subject, body, and the stat line", () => {
    const html = commitHeaderHtml(show());
    expect(html).toContain("add feature");
    expect(html).toContain("long explanation");
    expect(html).toContain("+3");
  });

  it("escapes body, author, and email", () => {
    const html = commitHeaderHtml(show({ body: "<script>x</script>", author: '"><i>a</i>', email: "<e>" }));
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<i>a</i>");
  });

  it("shows the truncation notice only for capped patches", () => {
    expect(commitHeaderHtml(show({ truncated: true }))).toContain("diff truncated");
    expect(commitHeaderHtml(show())).not.toContain("diff truncated");
  });

  // A patch kept in pages (#95) says it was cut only where nothing has said
  // what the whole weighs; one whose total is known shows how much is drawn at
  // the end of its stack instead, and is read on from there.
  it("says a paged patch was cut only where its whole weight is unknown", () => {
    const partial = show({ truncated: true, paged: true, pages: { end: 1_048_000, total: 4_000_000, complete: false } });
    expect(commitHeaderHtml(partial)).not.toContain("diff truncated");
    const unknown = show({ truncated: true, paged: true, pages: { end: 1_048_000, total: null, complete: false } });
    expect(commitHeaderHtml(unknown)).toContain("diff truncated at 1 MiB — the counts above are exact");
    const whole = show({ truncated: true, paged: true, pages: { end: 4_000_000, total: 4_000_000, complete: true } });
    expect(commitHeaderHtml(whole)).not.toContain("diff truncated");
  });
});

describe("commentTrayHtml", () => {
  const comments = [
    { id: 1, file: "src/a.js", lnA: 12, lnB: 14, snippet: "let x = 1;", comment: "rename this" },
    { id: 2, file: "src/b.js", lnA: 0, lnB: 0, snippet: "(entire file)", comment: "split it" },
  ];

  it("lists each pending comment with its location and a remove control", () => {
    const html = commentTrayHtml(comments);
    expect(html).toContain("src/a.js:12-14");
    expect(html).toContain("rename this");
    expect(html).toContain('data-id="2"');
    expect(html).toContain("split it");
    // a whole-file comment carries no line suffix
    expect(html).not.toContain("src/b.js:0");
  });

  it("draws nothing at all with nothing pending", () => {
    // Where a note is written is the box under the diff, which is always on
    // screen; an empty tray has nothing left to say.
    expect(commentTrayHtml([])).toBe("");
  });

  it("escapes comment text, snippets, and paths", () => {
    const html = commentTrayHtml([{ id: 1, file: "<img src=x>", lnA: 1, lnB: 1, snippet: "<b>s</b>", comment: "<i>c</i>" }], {});
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>s</b>");
    expect(html).not.toContain("<i>c</i>");
  });
});

describe("changesetPlaceholderHtml", () => {
  it("says what to do instead of rendering an empty pane", () => {
    expect(changesetPlaceholderHtml("Select a commit.")).toContain("Select a commit.");
  });

  it("escapes its message", () => {
    expect(changesetPlaceholderHtml("<img src=x>")).not.toContain("<img");
  });
});
