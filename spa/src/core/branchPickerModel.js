// Starting work on a branch, decided without a DOM.
//
// The create modal's Branch tab lists every branch the project has and lets the
// human press one. What pressing it does depends on where that branch already
// is: a branch a run holds is opened, a branch checked out somewhere Build does
// not own is adopted, a branch nothing holds is checked out into a new managed
// worktree (the bridge fetches it first when only a remote has it), and typed
// text that names no branch cuts build/<slug> the way this modal always has.
// The bridge names the holder; this module maps that name to a verb, a call and
// a landing place, and orders nothing of its own.

import { fuzzyRank } from "./fuzzy.js";
import { primaryAdoptScope, worktreeAdoptScope } from "./adoption.js";
import { branchNamePreview } from "./toolbarModel.js";

/** What each row promises, in the words the human reads on it. */
export const INTENT_VERB = {
  open: "Open",
  adopt: "Adopt",
  checkout: "Check out",
  materialise: "Fetch & check out",
  cut: "Create",
};

const worktreeCreate = (projectId, params) => ({ method: "worktree.create", params: { project_id: projectId, ...params } });

const runAdopt = (scope) => ({ method: "run.adopt", params: scope });

/**
 * What starting work on a branch means to each kind of checkout that can be
 * holding it. The bridge sends exactly one holder — it knows which of them
 * speaks for a branch two of them describe — so this is a lookup and never an
 * order of its own.
 */
const HOLDER_START = {
  run: () => ({ intent: "open", detail: "open in a run", call: null, emptyCheckout: false }),
  primary_checkout: (projectId) => ({
    intent: "adopt",
    detail: "checked out in the primary checkout",
    call: runAdopt(primaryAdoptScope(projectId)),
    emptyCheckout: false,
  }),
  external_worktree: (projectId, holder) => ({
    intent: "adopt",
    detail: "checked out in another worktree",
    call: runAdopt(worktreeAdoptScope(projectId, holder.id)),
    emptyCheckout: false,
  }),
};

/** What a holder this build has no name for offers: the branch is taken, so the
 *  row opens it and asks the bridge for nothing. The bridge ships apart from
 *  this client and can name a kind it was released before, and a held branch
 *  that read as unheld would send a checkout of a branch already checked out. */
const HELD_ELSEWHERE = { intent: "open", detail: "checked out elsewhere", call: null, emptyCheckout: false };

/** What starting work on this listed branch means: what its holder offers, or
 *  a checkout of the branch nothing holds. */
function branchStart(projectId, listed) {
  const holder = listed.holder;
  if (holder) {
    const held = HOLDER_START[holder.kind];
    return held ? held(projectId, holder) : HELD_ELSEWHERE;
  }
  return {
    intent: listed.remote ? "materialise" : "checkout",
    detail: "",
    call: worktreeCreate(projectId, { branch: listed.name }),
    emptyCheckout: true,
  };
}

/** Cutting build/<slug> after the typed text — this modal's original and only
 *  behaviour, kept whole. */
const cutStart = (projectId, query) => ({
  intent: "cut",
  detail: "",
  call: worktreeCreate(projectId, { name: query }),
  emptyCheckout: true,
});

/**
 * Where a pressed row lands. Every checkout of this project — a run, an
 * adopted worktree, the primary checkout, a branch just cut — is opened by the
 * branch it is on, so one route serves them all. A row with no branch of its
 * own learns it from the answer that made it.
 *
 * The composer is focused for a checkout with nobody in it, which opens on the
 * ghost composer where the first message belongs; a run already has a
 * conversation of its own.
 */
const landing = (projectId, branch, focusComposer) => (answer) => ({
  route: { name: "branch", projectId, branch: branch || (answer && answer.branch) || "", tab: "changes" },
  focusComposer,
});

/**
 * One pressable row, as the whole action pressing it takes: the verb it
 * promises, the call that keeps the promise, and where the answer lands. What
 * the start is stays behind it — the wiring presses a row without ever asking
 * what kind of row it is.
 */
const pressableRow = (projectId, { name, remote, branch, start }) => ({
  name,
  verb: INTENT_VERB[start.intent],
  detail: start.detail,
  remote: remote || null,
  call: start.call,
  land: landing(projectId, branch, start.emptyCheckout),
});

const branchRow = (projectId, listed) =>
  pressableRow(projectId, {
    name: listed.name,
    remote: listed.remote,
    branch: listed.name,
    start: branchStart(projectId, listed),
  });

/** The row that is not a branch of the project's: the branch this text would
 *  cut. Its branch is not known until the bridge answers with the name it
 *  slugified, so the row carries none. */
const cutNewRow = (projectId, query) =>
  pressableRow(projectId, {
    name: branchNamePreview(query),
    remote: null,
    branch: null,
    start: cutStart(projectId, query),
  });

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
 * The row an Enter press means. A press with nothing highlighted presses the
 * row the list leads with, which is the branch the text would cut whenever it
 * could cut one and the branch itself when the text spells one exactly. Text
 * that leaves no row standing, and an untouched field, press nothing — so
 * Enter there asks to be told a name rather than starting something unnamed.
 */
export function pressedRow({ rows = [], query = "", highlight = -1 }) {
  if (rows[highlight]) return rows[highlight];
  return String(query || "").trim() ? rows[0] || null : null;
}
