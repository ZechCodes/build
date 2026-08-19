// Pure markup builders for the git surface's REPO-MANAGEMENT chrome: the sync
// toolbar and the repo-state banner. The changeset markup — rail, headers,
// commit box, comment tray — lives in core/changesRender.js, and the diffs
// themselves in core/diffRender.js. No DOM — HTML strings only, unit-tested by
// string assertions. Every git-derived string (branch name, message) is esc()d.

import { esc } from "./text.js";

/** The exact canned instruction "Ask agent to commit" sends via task.message. */
export const AGENT_COMMIT_MESSAGE =
  "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.";

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
