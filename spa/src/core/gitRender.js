// Pure markup builders for the git surface's REPO-MANAGEMENT chrome: the branch
// control and its dropdown, the sync toolbar, and the repo-state banner. The
// changeset markup — rail, headers, commit box, comment tray — lives in
// core/changesRender.js, and the diffs themselves in core/diffRender.js.
// No DOM — HTML strings only, unit-tested by string assertions. Every
// git-derived string (branch name, message) is esc()d.

import { fuzzyRank } from "./fuzzy.js";
import { esc } from "./text.js";

/** The exact canned instruction "Ask agent to commit" sends via task.message. */
export const AGENT_COMMIT_MESSAGE =
  "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.";

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

/** One branch row's sync chips (↑/↓ against its own upstream, shown only when
 *  it tracks one) and its own diffstat against the project's base — what a
 *  reviewer would see switching onto it, whether or not it is checked out. */
function branchChipsHtml(branch) {
  const sync =
    branch.upstream && Number.isFinite(Number(branch.ahead)) && Number.isFinite(Number(branch.behind))
      ? `↑${esc(branch.ahead)} ↓${esc(branch.behind)}`
      : "";
  const stat = branch.stat || {};
  const insertions = Number(stat.insertions) || 0;
  const deletions = Number(stat.deletions) || 0;
  const weight = insertions || deletions ? `<span class="a">+${insertions}</span> <span class="d">−${deletions}</span>` : "";
  if (!sync && !weight) return "";
  return `<span class="gtbranch-chips">${sync}${sync && weight ? " " : ""}${weight}</span>`;
}

/** A branch checked out in a worktree Build has not adopted: picking it in the
 *  switcher cannot be a checkout, so the row says so and the client adopts
 *  that worktree instead. */
function branchElsewhereHtml(branch) {
  return branch.external_worktree_id ? `<span class="gtbranch-elsewhere">in another worktree</span>` : "";
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

/** How many branches the menu shows at once. The payload arrives current-first
 *  then most-recently-committed, so this is "the ones you were just on" — a repo
 *  with fifty branches made a dropdown taller than the window. Typing narrows the
 *  whole list, not just what is on screen. */
export const BRANCH_MENU_LIMIT = 8;

/** Move the keyboard cursor by `delta` over `count` rows, wrapping at both ends.
 *  Wrapping rather than clamping because the list is short and capped: ↑ from the
 *  top is the fastest way to the "Create …" row at the bottom. Returns 0 for an
 *  empty list, so the caller never has to special-case it. */
export function moveActiveIndex(current, delta, count) {
  if (!count || count < 1) return 0;
  const from = Number.isInteger(current) ? current : 0;
  return (((from + delta) % count) + count) % count;
}

/** The branch dropdown for a git.branches payload: one input that both filters
 *  (fuzzily) and names a new branch, the matching branches under it — capped,
 *  each with optional ahead/behind chips and a delete affordance — and a
 *  "Create …" row whenever what you typed is not already a branch.
 *  `null` payload → a loading placeholder. */
export function branchMenuHtml(
  payload,
  { pendingConfirm = null, forceDeleteOffered = [], query = "", activeIndex = 0 } = {},
) {
  if (!payload) return '<div class="gtbranch-menu"><div class="gtbranch-loading">loading…</div></div>';
  const branches = payload.branches || [];
  const search = String(query || "").trim();
  const matches = fuzzyRank(branches, search, (b) => b.name);
  const shown = matches.slice(0, BRANCH_MENU_LIMIT);
  const hidden = matches.length - shown.length;
  const exact = branches.some((b) => b.name === search);
  // Keyboard cursor: the rows and the create row form ONE list in DOM order, so
  // ↓ walks from the last branch onto "Create …" without a special case.
  const rows = shown
    .map(
      (b, i) => `<div class="gtbranch-item${b.is_current ? " current" : ""}${i === activeIndex ? " active" : ""}" data-branch="${esc(b.name)}"${
        b.external_worktree_id ? ` data-external-worktree-id="${esc(b.external_worktree_id)}"` : ""
      }>
        <span class="gtbranch-name">${esc(b.name)}</span>
        ${branchElsewhereHtml(b)}
        ${branchChipsHtml(b)}
        ${branchDeleteHtml(b, pendingConfirm, forceDeleteOffered)}</div>`,
    )
    .join("");
  // The create row is an ANSWER to what was typed, so it sits with the results
  // rather than in a separate form: no name, no row.
  const createRow =
    search && !exact
      ? `<div class="gtbranch-item gtbranch-create${shown.length === activeIndex ? " active" : ""}" data-branch="${esc(search)}">
          <span class="gtbranch-name">Create <strong>${esc(search)}</strong></span></div>`
      : "";
  // Say why the list is empty even when a create row follows: "no matching
  // branches" is the answer to what was typed; Create is what to do about it.
  const empty = rows ? "" : `<div class="gtbranch-empty">${search ? "no matching branches" : "no branches"}</div>`;
  return `<div class="gtbranch-menu">
    <div class="gtbranch-searchrow">
      <input class="gtbranch-newinput" type="text" placeholder="Search or name a new branch…" value="${esc(search)}" />
    </div>
    <div class="gtbranch-list">${rows}${empty}</div>
    ${createRow}
    ${hidden > 0 ? `<div class="gtbranch-more">${hidden} more — keep typing to narrow</div>` : ""}
  </div>`;
}
