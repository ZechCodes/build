// Pure markup builders for the git surface's REPO-MANAGEMENT chrome: the sync
// toolbar and the repo-state banner. The changeset markup — rail, headers,
// commit box, comment tray — lives in core/changesRender.js, and the diffs
// themselves in core/diffRender.js. No DOM — HTML strings only, unit-tested by
// string assertions. Every git-derived string (branch name, message) is esc()d.

import { esc } from "./text.js";
import { ICON_REFRESH } from "./icons.js";
import { selectionBarHtml } from "./changesRender.js";

/** The exact canned instruction "Ask agent to commit" sends via task.message. */
export const AGENT_COMMIT_MESSAGE =
  "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.";

/** Where the branch stands, on the left of the bar and never covered: how far
 *  it is from its upstream, and how much its working tree weighs. Facts, not
 *  verbs — which is why the banner that hides the verbs leaves these alone. */
function gitStatusHtml({ chips, stat }) {
  const sync = chips
    ? `<span class="gtchips"><span class="gtahead" title="ahead of upstream">↑${esc(chips.ahead)}</span> <span class="gtbehind" title="behind upstream">↓${esc(chips.behind)}</span></span>`
    : "";
  // A clean tree weighs +0 −0, and saying so is not the same as saying nothing:
  // only a status this client never received is absent.
  const weight = stat
    ? `<span class="pm gtweight"><span class="a">+${esc(Number(stat.insertions) || 0)}</span> <span class="d">−${esc(Number(stat.deletions) || 0)}</span></span>`
    : "";
  return sync || weight ? `<div class="gtstatus">${sync}${weight}</div>` : "";
}

/** The repo-management verbs: Fetch, the ahead/behind-driven Pull and Push
 *  hosts, and Stash. Absent against a bridge too old to report `repo_state` —
 *  every control degrades to hidden rather than rendering NaN. */
function gitRepoVerbsHtml() {
  return `<div class="gtsync">
      <button class="btn mini gtfetch" title="Fetch --prune">${ICON_REFRESH} Fetch</button>
      <div class="gtpull"></div>
      <div class="gtpush"></div>
    </div>
    <div class="gtstash"></div>`;
}

/** The bulk verbs the selection raises, behind a divider that says they belong
 *  to something else. They live in the BAR rather than over the stack so a
 *  reviewer deep in a long diff can still reach them. */
function gitSelectionHtml(selected) {
  if (!selected) return "";
  return `<span class="gtdivider" aria-hidden="true"></span>${selectionBarHtml(selected)}`;
}

/**
 * The git bar: where the branch stands, then what can be done to it.
 *
 * Left to right — the status, the repo verbs, the surface's own merge verb,
 * and, only while files are in hand, the verbs aimed at those. Everything sits
 * above the scroller, so scrolling through a long diff never takes a number or
 * a button off the screen.
 *
 * A `banner` — a repository mid-merge, mid-rebase, conflicted — takes the whole
 * bar past the status. It has one thing to say and one thing to do, and the
 * verbs it replaces are the ones that must not be pressed until it is over, so
 * they are gone rather than merely covered: a button nobody can see is still a
 * button a keyboard can reach.
 *
 * `repo` false draws the bar with the status and the merge host alone: an older
 * bridge reports no repo_state, but the surface's own verbs do not depend on
 * the bridge's vintage.
 */
export function gitToolbarHtml({ chips, stat = null, selected = 0, banner = null, pendingConfirm = null, repo = true }) {
  const rest = banner
    ? gitStateBannerHtml(banner, { pendingConfirm })
    : `${repo ? gitRepoVerbsHtml() : ""}<div class="gtmerge"></div>${gitSelectionHtml(selected)}`;
  return `<div class="gittoolbar">${gitStatusHtml({ chips, stat })}<div class="gtrest">${rest}</div></div>`;
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
