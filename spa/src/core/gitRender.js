// Pure markup builders for the git surface (the Changes tab on the primary
// checkout and on coding-session worktrees): the uncommitted-changes block with
// per-file stage checkboxes, the commit-history rows, and the expanded commit
// detail. No DOM — HTML strings only, unit-tested by string assertions. Every
// git-derived string (subject, body, author, email, branch, path) is esc()d.

import { esc, humanAge } from "./text.js";
import { parseDiff, filterNoiseFiles } from "./diff.js";
import { diffFilesHtml, diffRowsHtml } from "./diffRender.js";

/** The exact canned instruction "Ask agent to commit" sends via task.message. */
export const AGENT_COMMIT_MESSAGE =
  "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.";

const TRUNCATED_NOTICE = '<div class="ftrunc">diff truncated at 1 MiB — the stat above is exact</div>';

/** Map a git.status file entry onto the diff surface's .fb badge vocabulary.
 *  Conflicted entries ("U") reuse the DEL styling under a CONFLICT label. */
function fileBadge(file) {
  if (file.index_status === "U" || file.worktree_status === "U") return { cls: "DEL", label: "CONFLICT" };
  if (file.index_status === "D" || file.worktree_status === "D") return { cls: "DEL", label: "DEL" };
  if (file.index_status === "?" || file.index_status === "A" || file.worktree_status === "A") return { cls: "ADD", label: "ADD" };
  return { cls: "EDIT", label: "EDIT" };
}

const statSummary = (stat) =>
  `<span class="gitstat">${stat.files_changed} files <span class="a">+${stat.insertions}</span> <span class="d">−${stat.deletions}</span></span>`;

/** The uncommitted-changes block for a git.status payload: per-file stage rows
 *  (.toggle-wrapped native checkbox + path + .fb badge) with the file's diff
 *  rows beneath, a truncation notice when the patch was capped, and the
 *  commit-message box + actions host. Empty status → placeholder, no box. */
export function uncommittedHtml(status) {
  const files = status.files || [];
  const stat = status.stat || { files_changed: 0, insertions: 0, deletions: 0 };
  if (!files.length)
    return `<div class="gitsec"><div class="gitsec-head">Uncommitted changes</div>
      <div class="empty">No uncommitted changes.</div>
      <div class="actionbar"><span class="hint githint"></span></div></div>`;
  const diffByPath = new Map(filterNoiseFiles(parseDiff(status.patch)).map((f) => [f.path, f]));
  const rows = files
    .map((f) => {
      const badge = fileBadge(f);
      const checked = f.staged === "full" ? " checked" : "";
      const diffFile = diffByPath.get(f.path);
      return `<div class="file gitfile"><div class="fhead">
        <label class="toggle"><input type="checkbox" class="stagebox" data-path="${esc(f.path)}"${checked}></label>
        <span>${esc(f.path)}</span><span class="fb ${badge.cls}">${badge.label}</span>
        ${diffFile ? `<span class="pm"><span class="a">+${diffFile.add}</span> <span class="d">−${diffFile.del}</span></span>` : ""}</div>
        ${diffFile ? `<table>${diffRowsHtml(diffFile.rows)}</table>` : ""}</div>`;
    })
    .join("");
  return `<div class="gitsec">
    <div class="gitsec-head">Uncommitted changes ${statSummary(stat)}</div>
    ${rows}
    ${status.files_truncated ? `<div class="ftrunc">file list truncated — ${files.length} shown</div>` : ""}
    ${status.truncated ? TRUNCATED_NOTICE : ""}
    <div class="gitcommit">
      <textarea class="gitmsg" placeholder="Commit message…"></textarea>
      <div class="actionbar"><span class="hint githint"></span><div class="right gitcommit-actions"></div></div>
    </div></div>`;
}

/** One commit-history row: short hash (mono), subject, author, relative age.
 *  Task-scope commits ahead of the base branch carry the `ahead` class. */
export function commitRowHtml(commit, { expanded = false, nowSeconds = Date.now() / 1000 } = {}) {
  const classes = ["crow", commit.ahead_of_base ? "ahead" : "", expanded ? "expanded" : ""].filter(Boolean).join(" ");
  return `<div class="${classes}" data-hash="${esc(commit.hash)}">
    <span class="chash">${esc(commit.short)}</span>
    <span class="csubject">${esc(commit.subject)}</span>
    <span class="cauthor">${esc(commit.author)}</span>
    <span class="cage">${esc(humanAge(nowSeconds - (commit.time || 0)))}</span></div>`;
}

/** The expanded commit detail for a git.show payload: subject, body, stat line,
 *  truncation notice when capped, and the commit's parsed diff. */
export function commitDetailHtml(show) {
  const files = filterNoiseFiles(parseDiff(show.patch));
  const stat = show.stat || { files_changed: 0, insertions: 0, deletions: 0 };
  const body = (show.body || "").trim();
  return `<div class="cdetail" data-hash="${esc(show.hash)}">
    <div class="csub">${esc(show.subject)}</div>
    ${body ? `<pre class="cbody">${esc(body)}</pre>` : ""}
    <div class="cinfo">${esc(show.short)} · ${esc(show.author)} &lt;${esc(show.email)}&gt; · ${statSummary(stat)}</div>
    ${show.truncated ? TRUNCATED_NOTICE : ""}
    ${diffFilesHtml(files)}</div>`;
}

/** The commit-history section for a git.log payload (plus any paged-in extra
 *  commits merged by the caller). The expanded row inlines its cached git.show
 *  detail, or a loading placeholder while the fetch is in flight. */
export function historyHtml(log, { expandedHash = null, expandedDetail = null, nowSeconds = Date.now() / 1000 } = {}) {
  const commits = log.commits || [];
  const rows = commits
    .map((c) => {
      const expanded = c.hash === expandedHash;
      let row = commitRowHtml(c, { expanded, nowSeconds });
      if (expanded) row += expandedDetail ? commitDetailHtml(expandedDetail) : '<div class="cdetail cdetail-loading">loading…</div>';
      return row;
    })
    .join("");
  return `<div class="gitsec">
    <div class="gitsec-head">History</div>
    ${commits.length ? `<div class="clist">${rows}</div>` : '<div class="empty">No commits yet.</div>'}
    ${log.more ? '<div class="cmore"><button class="btn mini gitmore">Show more</button></div>' : ""}</div>`;
}
