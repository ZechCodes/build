/**
 * What a project with no git is offered where a verb needed git (#297). The
 * bridge refuses branches, diffs and git checkouts on a plain folder in one
 * sentence; a client that meets it offers to initialize Git instead of
 * printing it, and nothing initializes Git without the user asking.
 */
import { esc, messageOf } from "./text.js";

const NO_GIT_REFUSAL = "project is not a git repository";

/** Whether a bridge refusal is the one a plain folder's project answers with. */
export const refusedForNoGit = (error) => messageOf(error).includes(NO_GIT_REFUSAL);

/** The compose box's offer, in place of the refusal: `error` is why the last
 *  attempt to initialize did not, and `pending` holds the button while one runs. */
export function composeGitOfferHtml({ error = "", pending = false }) {
  return `<div class="compose-git-offer" role="group" aria-label="Initialize Git">
    <p>This project's folder has no Git yet, and a branch needs it. Initialize Git to track changes and create branches in this folder.</p>
    <button class="btn mini primary" type="button" data-compose-init-git${pending ? " disabled" : ""}>Initialize Git</button>
    <p class="error" role="status" data-compose-init-git-status>${esc(error)}</p>
  </div>`;
}
