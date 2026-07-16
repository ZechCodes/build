import { describe, it, expect } from "vitest";
import {
  AGENT_COMMIT_MESSAGE,
  uncommittedHtml,
  commitRowHtml,
  commitDetailHtml,
  changesRailHtml,
} from "../src/core/gitRender.js";

const NOW = 1_750_000_000; // fixed clock for relative-time assertions

const patchFor = (path, addedLine) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old line\n+${addedLine}\n`;

const status = (overrides = {}) => ({
  branch: "main",
  path: "/repo",
  head: "f".repeat(40),
  files: [
    { path: "src/a.js", staged: "full", index_status: "M", worktree_status: "-" },
    { path: "notes.txt", staged: "none", index_status: "?", worktree_status: "?" },
    { path: "core/b.py", staged: "partial", index_status: "M", worktree_status: "M" },
  ],
  stat: { files_changed: 3, insertions: 5, deletions: 2 },
  patch: patchFor("src/a.js", "new line") + patchFor("core/b.py", "py line"),
  truncated: false,
  ...overrides,
});

const commit = (overrides = {}) => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "fix the widget",
  author: "Zech",
  email: "z@example.com",
  time: NOW - 120,
  ...overrides,
});

describe("AGENT_COMMIT_MESSAGE", () => {
  it("is the exact canned agent-commit instruction", () => {
    expect(AGENT_COMMIT_MESSAGE).toBe(
      "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.",
    );
  });
});

describe("uncommittedHtml", () => {
  it("renders a toggle-wrapped stage checkbox per file with staged state", () => {
    const html = uncommittedHtml(status());
    expect(html).toContain('class="toggle');
    // full → checked, none → unchecked
    expect(html).toContain('data-path="src/a.js" checked');
    expect(html).toContain('data-path="notes.txt">');
    expect(html).toContain('data-path="core/b.py"');
    expect((html.match(/class="stagebox"/g) || []).length).toBe(3);
  });

  it("badges untracked files ADD and modified files EDIT via .fb classes", () => {
    const html = uncommittedHtml(status());
    expect(html).toContain('class="fb ADD"');
    expect(html).toContain('class="fb EDIT"');
  });

  it("badges deletions DEL", () => {
    const html = uncommittedHtml(
      status({ files: [{ path: "gone.txt", staged: "none", index_status: "M", worktree_status: "D" }] }),
    );
    expect(html).toContain('class="fb DEL"');
  });

  it("badges conflicted files CONFLICT with the DEL styling", () => {
    const html = uncommittedHtml(
      status({ files: [{ path: "f.txt", staged: "none", index_status: "U", worktree_status: "U" }] }),
    );
    expect(html).toContain('class="fb DEL">CONFLICT<');
  });

  it("shows a quiet notice when the file list was truncated", () => {
    expect(uncommittedHtml(status({ files_truncated: true }))).toContain("file list truncated — 3 shown");
    expect(uncommittedHtml(status())).not.toContain("file list truncated");
  });

  it("renders each file's diff rows beneath its stage row (syntax-highlighted)", () => {
    const html = uncommittedHtml(status());
    // Code cells are now Prism-highlighted, so the text may be split across
    // token spans (e.g. "new" is a JS keyword); assert the content survives and
    // still lives in a td.code cell rather than pinning the exact inner markup.
    const stripped = html.replace(/<[^>]+>/g, "");
    expect(stripped).toContain("new line");
    expect(stripped).toContain("py line");
    expect(html).toContain('<td class="code">');
  });

  it("filters noise paths out of the rendered patch", () => {
    const html = uncommittedHtml(
      status({
        files: [{ path: "uv.lock", staged: "none", index_status: "?", worktree_status: "?" }],
        patch: patchFor("uv.lock", "locked"),
      }),
    );
    expect(html).not.toContain("locked");
    // The stage row itself still shows — the file is stageable, only its diff is noise.
    expect(html).toContain('data-path="uv.lock"');
  });

  it("escapes git-derived paths (an XSS path renders inert)", () => {
    const evil = '<img src=x onerror=alert(1)>.txt';
    const html = uncommittedHtml(
      status({ files: [{ path: evil, staged: "none", index_status: "?", worktree_status: "?" }], patch: "" }),
    );
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });

  it("shows a truncation notice when the status patch was capped", () => {
    expect(uncommittedHtml(status({ truncated: true }))).toContain("diff truncated");
    expect(uncommittedHtml(status())).not.toContain("diff truncated");
  });

  it("includes the commit-message box and actions host when there are changes", () => {
    const html = uncommittedHtml(status());
    expect(html).toContain('class="gitmsg"');
    expect(html).toContain('class="right gitcommit-actions"');
    expect(html).toContain('class="hint githint"');
  });

  it("renders the empty state with no commit box when clean", () => {
    const html = uncommittedHtml(status({ files: [], patch: "", stat: { files_changed: 0, insertions: 0, deletions: 0 } }));
    expect(html).toContain("No uncommitted changes.");
    expect(html).not.toContain('class="gitmsg"');
  });

  it("keeps a hint host in the pane even when the tree is clean", () => {
    const html = uncommittedHtml(status({ files: [], patch: "", stat: { files_changed: 0, insertions: 0, deletions: 0 } }));
    expect(html).toContain('class="hint githint"');
  });
});

describe("commitRowHtml", () => {
  it("renders short hash, subject, author, and relative time", () => {
    const html = commitRowHtml(commit(), { nowSeconds: NOW });
    expect(html).toContain(`data-hash="${"a".repeat(40)}"`);
    expect(html).toContain('class="chash">aaaaaaa<');
    expect(html).toContain("fix the widget");
    expect(html).toContain("Zech");
    expect(html).toContain("2m ago");
  });

  it("escapes a malicious subject and author", () => {
    const html = commitRowHtml(commit({ subject: '<img src=x onerror=alert(1)>', author: "<b>evil</b>" }), { nowSeconds: NOW });
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>evil</b>");
    expect(html).toContain("&lt;img");
  });

  it("marks commits ahead of the base branch", () => {
    expect(commitRowHtml(commit({ ahead_of_base: true }), { nowSeconds: NOW })).toContain("ahead");
    expect(commitRowHtml(commit(), { nowSeconds: NOW })).not.toContain("ahead");
  });

  it("marks the selected row", () => {
    expect(commitRowHtml(commit(), { selected: true, nowSeconds: NOW })).toContain("sel");
    expect(commitRowHtml(commit(), { nowSeconds: NOW })).not.toContain("sel");
  });
});

describe("commitDetailHtml", () => {
  const show = (overrides = {}) => ({
    hash: "b".repeat(40),
    short: "bbbbbbb",
    subject: "add feature",
    body: "long explanation",
    author: "Zech",
    email: "z@example.com",
    time: NOW - 3600,
    stat: { files_changed: 1, insertions: 3, deletions: 1 },
    patch: patchFor("src/a.js", "detail line"),
    truncated: false,
    ...overrides,
  });

  it("renders subject, body, stat, and the parsed diff", () => {
    const html = commitDetailHtml(show());
    expect(html).toContain("add feature");
    expect(html).toContain("long explanation");
    expect(html).toContain("+3");
    expect(html).toContain('<td class="code">detail line</td>');
  });

  it("escapes body, author, and email", () => {
    const html = commitDetailHtml(show({ body: "<script>x</script>", author: '"><i>a</i>', email: "<e>" }));
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<i>a</i>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("shows the truncation notice only for capped patches", () => {
    expect(commitDetailHtml(show({ truncated: true }))).toContain("diff truncated");
    expect(commitDetailHtml(show())).not.toContain("diff truncated");
  });

  it("filters noise files from the commit's patch", () => {
    const html = commitDetailHtml(show({ patch: patchFor(".build/task.json", "meta") }));
    expect(html).not.toContain("meta");
  });
});

describe("changesRailHtml", () => {
  const log = (overrides = {}) => ({
    branch: "main",
    commits: [commit(), commit({ hash: "c".repeat(40), short: "ccccccc", subject: "older" })],
    more: false,
    ...overrides,
  });
  const rail = (overrides = {}) =>
    changesRailHtml({ review: null, status: status(), log: log(), selected: "uncommitted", nowSeconds: NOW, ...overrides });

  it("renders a commit row per log entry under a History head", () => {
    const html = rail();
    expect(html).toContain(`data-hash="${"a".repeat(40)}"`);
    expect(html).toContain(`data-hash="${"c".repeat(40)}"`);
    expect(html).toContain("History");
  });

  it("always offers the Uncommitted entry, with the dirty file count", () => {
    expect(rail()).toContain('data-sel="uncommitted"');
    expect(rail()).toContain("3 files");
    expect(rail({ status: status({ files: [] }) })).toContain("clean");
  });

  it("offers the review entry only when a review context exists, showing the base", () => {
    expect(rail()).not.toContain('data-sel="review"');
    const html = rail({ review: { base: "main" }, selected: "review" });
    expect(html).toContain('data-sel="review"');
    expect(html).toContain("All changes");
    expect(html).toContain("vs main");
  });

  it("escapes the review base branch", () => {
    const html = rail({ review: { base: '<img src=x>' } });
    expect(html).not.toContain("<img");
  });

  it("marks exactly the selected entry", () => {
    const html = rail({ selected: "a".repeat(40) });
    expect(html).toContain('crow sel"');
    expect(html.match(/rrow sel/g)).toBeNull();
  });

  it("shows the Show more affordance only when another page exists", () => {
    expect(rail({ log: log({ more: true }) })).toContain('class="btn mini gitmore"');
    expect(rail()).not.toContain("gitmore");
  });

  it("renders the empty state for a repo with no commits", () => {
    expect(rail({ log: log({ commits: [] }) })).toContain("No commits yet.");
  });
});
