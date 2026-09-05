// Starting work on a branch, decided without a DOM.
//
// The create modal's Branch tab lists every branch the project has and lets the
// human press one. What pressing it does depends on where that branch already
// is: a branch a run owns is opened, a branch checked out somewhere Build does
// not own is adopted, a branch nothing holds is checked out into a new managed
// worktree (the bridge fetches it first when only a remote has it), and typed
// text that names no branch cuts build/<slug> the way this modal always has.
//
// Every one of those answers is built here, once, at the moment a row is made:
// a row carries its own verb, its own call and its own landing place, so the
// wiring presses a row without ever asking what kind of row it is.

import { fuzzyRank } from "./fuzzy.js";
import { branchNamePreview } from "./toolbarModel.js";

/** What each row promises, in the words the human reads on it. */
export const INTENT_VERB = {
  open: "Open",
  adopt: "Adopt",
  checkout: "Check out",
  materialise: "Fetch & check out",
  cut: "Create",
};

/** The one row that is not a branch: the branch this text would cut. */
const CUT_NEW_KEY = "create-new";

const worktreeCreate = (projectId, params) => ({ method: "worktree.create", params: { project_id: projectId, ...params } });

const runAdopt = (projectId, scope) => ({ method: "run.adopt", params: { project_id: projectId, ...scope } });

/**
 * What starting work on this listed branch means. The first holder that claims
 * it answers — a run knows the branch's whole life, the primary checkout knows
 * it is the repository, and a worktree Build never cut knows only that git has
 * the branch open there — and a branch nothing holds is one to check out.
 */
function branchStart(projectId, listed) {
  if (listed.run_id) return { intent: "open", detail: "open in a run", call: null, emptyCheckout: false };
  if (listed.primary_worktree_id) {
    return {
      intent: "adopt",
      detail: "checked out in the primary checkout",
      call: runAdopt(projectId, { primary: true }),
      emptyCheckout: false,
    };
  }
  if (listed.external_worktree_id) {
    return {
      intent: "adopt",
      detail: "checked out in another worktree",
      call: runAdopt(projectId, { worktree_id: listed.external_worktree_id }),
      emptyCheckout: false,
    };
  }
  return {
    intent: listed.remote ? "materialise" : "checkout",
    detail: "",
    call: worktreeCreate(projectId, { branch: listed.name }),
    emptyCheckout: true,
  };
}

function branchRow(projectId, listed) {
  const start = branchStart(projectId, listed);
  return {
    key: `branch:${listed.name}`,
    name: listed.name,
    intent: start.intent,
    verb: INTENT_VERB[start.intent],
    detail: start.detail,
    remote: listed.remote || null,
    isCurrent: !!listed.is_current,
    ahead: Number(listed.ahead) || 0,
    behind: Number(listed.behind) || 0,
    branch: listed.name,
    // A checkout with nobody in it opens on the ghost composer, which is where
    // the first message belongs; a run already has a conversation of its own.
    focusComposer: start.emptyCheckout,
    call: start.call,
  };
}

/**
 * The row that cuts a new branch after `query` — this modal's original and
 * only behaviour, kept whole. Its branch is not known until the bridge answers
 * with the name it slugified, so the row carries none.
 */
export function cutNewRow(projectId, query) {
  return {
    key: CUT_NEW_KEY,
    name: branchNamePreview(query),
    intent: "cut",
    verb: INTENT_VERB.cut,
    detail: "",
    remote: null,
    isCurrent: false,
    ahead: 0,
    behind: 0,
    branch: null,
    focusComposer: true,
    call: worktreeCreate(projectId, { name: query }),
  };
}

/**
 * The rows the typed text leaves standing, most likely first.
 *
 * `branches` is `git.branches`'s listing, already ordered current-first then by
 * recency; ranking keeps that order for ties. The cut-new row leads whenever
 * the text could cut a branch and names none of the project's own — typing a
 * branch's exact name means that branch, not a second one beside it.
 */
export function branchPickerRows({ projectId, branches = [], query = "" }) {
  const listing = branches || [];
  const typed = String(query || "").trim();
  const rows = fuzzyRank(listing, typed, (listed) => listed.name).map((listed) => branchRow(projectId, listed));
  const cuts = typed && branchNamePreview(typed) && !listing.some((listed) => listed.name === typed);
  return cuts ? [cutNewRow(projectId, query), ...rows] : rows;
}

/**
 * Where a pressed row lands. Every checkout of this project — a run, an
 * adopted worktree, the primary checkout, a branch just cut — is opened by the
 * branch it is on, so one route serves them all. The cut-new row learns its
 * branch from the answer that cut it.
 */
export function branchStartRoute(projectId, row, answer) {
  return {
    name: "branch",
    projectId,
    branch: row.branch || (answer && answer.branch) || "",
    tab: "changes",
  };
}
