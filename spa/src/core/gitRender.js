// Pure markup builders for the git surface (the Changes tab on the primary
// checkout and on coding-session worktrees): the uncommitted-changes block with
// per-file stage checkboxes, the commit-history rows, and the expanded commit
// detail. No DOM — HTML strings only, unit-tested by string assertions. Every
// git-derived string (subject, body, author, email, branch, path) is esc()d.

import { esc, humanAge } from "./text.js";
import { parseDiff, filterNoiseFiles } from "./diff.js";
import { diffFilesHtml, diffRowsHtml } from "./diffRender.js";
import { langForPath } from "./highlight.js";
import { isDotenvPath } from "./secrets.js";

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

/** The per-file discard affordance (repo-controls scope only): a small danger
 *  button that arms to a "Discard changes?" inline confirm when its path is the
 *  pending one. Absent entirely on an older bridge (repoControls false). */
function discardButtonHtml(path, pendingConfirm) {
  const armed = pendingConfirm === `discard:${path}`;
  return `<button class="btn mini danger gitdiscard${armed ? " armed" : ""}" data-path="${esc(path)}">${armed ? "Discard changes?" : "discard"}</button>`;
}

/** The uncommitted-changes block for a git.status payload: per-file stage rows
 *  (.toggle-wrapped native checkbox + path + .fb badge) with the file's diff
 *  rows beneath, a truncation notice when the patch was capped, and the
 *  commit-message box + actions host. Empty status → placeholder, no box.
 *  `repoControls` adds the per-file discard affordance; `pendingConfirm` arms
 *  the matching path's confirm. */
export function uncommittedHtml(status, { repoControls = false, pendingConfirm = null } = {}) {
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
      return `<div class="file gitfile capped"><div class="fhead">
        <label class="toggle"><input type="checkbox" class="stagebox" data-path="${esc(f.path)}"${checked}></label>
        <span class="fpath">${esc(f.path)}</span><span class="fb ${badge.cls}">${badge.label}</span>
        ${diffFile ? `<span class="pm"><span class="a">+${diffFile.add}</span> <span class="d">−${diffFile.del}</span></span>` : ""}
        ${repoControls ? discardButtonHtml(f.path, pendingConfirm) : ""}</div>
        ${diffFile ? `<div class="dscroll"><table>${diffRowsHtml(diffFile.rows, langForPath(f.path), { maskDotenv: isDotenvPath(f.path) })}</table></div><div class="diff-expand" aria-hidden="true">Expand full diff ↓</div>` : ""}</div>`;
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

/** One commit rail row: subject over short hash · author · relative age.
 *  Task-scope commits ahead of the base branch carry the `ahead` class; the
 *  rail's current selection carries `sel`. */
export function commitRowHtml(commit, { selected = false, nowSeconds = Date.now() / 1000 } = {}) {
  const classes = ["crow", commit.ahead_of_base ? "ahead" : "", selected ? "sel" : ""].filter(Boolean).join(" ");
  return `<div class="${classes}" data-hash="${esc(commit.hash)}">
    <span class="csubject">${esc(commit.subject)}</span>
    <span class="cmeta"><span class="chash">${esc(commit.short)}</span> · <span class="cauthor">${esc(commit.author)}</span> · <span class="cage">${esc(humanAge(nowSeconds - (commit.time || 0)))}</span></span></div>`;
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

/** The Changes tab's left rail: the pinned selections (the review "All
 *  changes" entry when the surface has one, then "Uncommitted"), and the
 *  commit history (plus any paged-in extra commits merged by the caller) with
 *  its Show more affordance. `selected` is "review" | "uncommitted" | a commit
 *  hash. `review` is { base } or null. */
export function changesRailHtml({ review = null, status, log, selected, branchControlHtml = "", nowSeconds = Date.now() / 1000 }) {
  const rrow = (sel, title, sub) =>
    `<div class="${["rrow", selected === sel ? "sel" : ""].filter(Boolean).join(" ")}" data-sel="${sel}">
      <span class="rtitle">${title}</span><span class="rsub mono">${sub}</span></div>`;
  const reviewRow = review ? rrow("review", "All changes", `vs ${esc(review.base || "main")}`) : "";
  const fileCount = (status.files || []).length;
  const uncommittedRow = rrow(
    "uncommitted",
    "Uncommitted",
    fileCount ? `${fileCount} file${fileCount === 1 ? "" : "s"}` : "clean",
  );
  const commits = (log.commits || [])
    .map((c) => commitRowHtml(c, { selected: selected === c.hash, nowSeconds }))
    .join("");
  return `<div class="crail">
    ${branchControlHtml ? `<div class="crail-branch">${branchControlHtml}</div>` : ""}
    ${reviewRow}${uncommittedRow}
    <div class="rhead">History</div>
    ${commits || '<div class="empty">No commits yet.</div>'}
    ${log.more ? '<div class="gitmore" role="button" tabindex="0">Load older commits…</div>' : ""}</div>`;
}

/** Branch selection belongs to the commit/history rail, not over the diff. */
export function gitBranchControlHtml({ branch, showBranchControl, branchMenuHtml: branchMenu = "" }) {
  return showBranchControl
    ? `<button class="btn mini gtbranchbtn" title="Switch branch">⑂ ${esc(branch || "(detached)")} ▾</button>${branchMenu}`
    : `<span class="gtbranchlabel">⑂ ${esc(branch || "(detached)")}</span>`;
}

/** The repo-management toolbar: Fetch + ahead/behind chips + Pull/Push split
 *  button hosts, and a Stash split-button host. Branch selection lives above
 *  the commit log in the rail. */
export function gitToolbarHtml({ chips }) {
  const chipsHtml = chips
    ? `<span class="gtchips"><span class="gtahead" title="ahead of upstream">↑${esc(chips.ahead)}</span> <span class="gtbehind" title="behind upstream">↓${esc(chips.behind)}</span></span>`
    : "";
  return `<div class="gittoolbar">
    <div class="gtsync">
      <button class="btn mini gtfetch" title="Fetch --prune"><span aria-hidden="true">↻</span> Fetch</button>
      ${chipsHtml}
      <div class="gtpull"></div>
      <div class="gtpush"></div>
    </div>
    <div class="gtstash"></div>
  </div>`;
}

/** Render a repo-state banner decision (repoStateBanner's { message, abortable }
 *  from gitPane — the single source of the copy) as markup: the situation plus
 *  an inline-confirm Abort for the abortable states. Empty string when the
 *  decision is null (clean repo, or an older bridge that omits repo_state). */
export function gitStateBannerHtml(banner, { pendingConfirm = null } = {}) {
  if (!banner) return "";
  const armed = pendingConfirm === "abort";
  const abort = banner.abortable
    ? `<button class="btn mini danger gitabort${armed ? " armed" : ""}">${armed ? "Confirm abort?" : "Abort"}</button>`
    : "";
  return `<div class="gitstate"><span class="gitstate-msg">${esc(banner.message)}</span>${abort}</div>`;
}

/** One branch row's ahead/behind chips — shown only when the branch tracks an
 *  upstream and both counts are numbers. */
function branchChipsHtml(branch) {
  if (!branch.upstream || !Number.isFinite(Number(branch.ahead)) || !Number.isFinite(Number(branch.behind))) return "";
  return `<span class="gtbranch-chips">↑${esc(branch.ahead)} ↓${esc(branch.behind)}</span>`;
}

/** One branch row's delete affordance: hidden for the current branch, a plain
 *  inline-confirm delete otherwise, upgraded to a force-delete (still confirmed)
 *  once a non-force delete has failed for that branch. */
function branchDeleteHtml(branch, pendingConfirm, forceDeleteOffered) {
  if (branch.is_current) return "";
  if (forceDeleteOffered.includes(branch.name)) {
    const armed = pendingConfirm === `branch_delete_force:${branch.name}`;
    return `<button class="gtbranch-del force${armed ? " armed" : ""}" data-branch="${esc(branch.name)}" data-force="1">${armed ? "Force delete?" : "force delete"}</button>`;
  }
  const armed = pendingConfirm === `branch_delete:${branch.name}`;
  return `<button class="gtbranch-del${armed ? " armed" : ""}" data-branch="${esc(branch.name)}">${armed ? "Delete?" : "delete"}</button>`;
}

/** The branch dropdown for a git.branches payload: the current branch marked,
 *  each with optional ahead/behind chips and a delete affordance, plus a
 *  new-branch input + Create row. `null` payload → a loading placeholder. */
export function branchMenuHtml(payload, { pendingConfirm = null, forceDeleteOffered = [] } = {}) {
  if (!payload) return '<div class="gtbranch-menu"><div class="gtbranch-loading">loading…</div></div>';
  const branches = payload.branches || [];
  const rows = branches
    .map(
      (b) => `<div class="gtbranch-item${b.is_current ? " current" : ""}" data-branch="${esc(b.name)}">
        <span class="gtbranch-name">${esc(b.name)}</span>
        ${branchChipsHtml(b)}
        ${branchDeleteHtml(b, pendingConfirm, forceDeleteOffered)}</div>`,
    )
    .join("");
  return `<div class="gtbranch-menu">
    ${rows || '<div class="gtbranch-empty">no branches</div>'}
    <div class="gtbranch-newrow">
      <input class="gtbranch-newinput" type="text" placeholder="new branch name" />
      <button class="btn mini gtbranch-create">Create</button>
    </div>
  </div>`;
}
