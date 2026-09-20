// The Issues tab's filters: state, status, assignee and label.
//
// They match `issues.list`'s params one for one, so a filter is a PARAM and not
// a client-side pass over everything — the bridge answers the narrowed list.
// The local pass below is for the other half of the same act: the cache holds
// the project's whole list (core/trackerCache.js), so the first paint applies
// the same filters to what is already on disk while the narrowed read is still
// in flight. Two readings of one rule, so they are written once here.
//
// No DOM, no app imports.

import { UNASSIGNED, assigneeFromKey, assigneeKey, columnsOf } from "./trackerModel.js";

/** Nothing narrowed: every issue of the project, open and closed. What the
 *  WHOLE list is read with — the menus are built from it, and a menu offering
 *  only what the current narrowing left could not offer Closed at all. */
export const NO_FILTERS = Object.freeze({ state: "", status: "", assignee: "", label: "" });

/**
 * What the tab opens on, and what Clear goes back to.
 *
 * Zech, deciding the question #28 raised: "If closed means done we shouldn't
 * show them in the default view." So Open is the default and closed issues are
 * a thing you ask for — the filter still offers Closed and Open and closed,
 * and a closed row wears its own chip when one is on screen
 * (core/trackerListRender.js), so it can never be misread as open.
 *
 * Not a narrowing of NO_FILTERS but a starting point beside it: "is this list
 * narrowed" is a question about what the reader asked for beyond the default,
 * which is what `filtersAreSet` answers below.
 */
export const DEFAULT_FILTERS = Object.freeze({ ...NO_FILTERS, state: "open" });

/** The word that means "assigned to somebody, no matter who". `issues.list`
 *  takes it beside the actor shapes and beside `"none"`. */
export const ANY_ASSIGNEE = "any";

/** The assignee param a filter value becomes: the two words ride as words, an
 *  actor key rides as the tagged shape, and an empty filter sends nothing —
 *  absent means "both" for state and "anyone" here. */
function assigneeParam(value) {
  if (!value) return undefined;
  if (value === ANY_ASSIGNEE || value === UNASSIGNED) return value;
  return assigneeFromKey(value) || undefined;
}

/** The filters as `issues.list` params. A filter nobody set is left off rather
 *  than sent empty: the verb reads an absent param as "do not narrow on this",
 *  and an empty string is not that. */
export function issueListParams(projectId, filters = {}) {
  const params = { project_id: projectId };
  if (filters.state) params.state = filters.state;
  if (filters.status) params.status = filters.status;
  if (filters.label) params.label = filters.label;
  const assignee = assigneeParam(filters.assignee);
  if (assignee !== undefined) params.assignee = assignee;
  return params;
}

const assigneeMatches = (assignee, want) => {
  if (!want) return true;
  if (want === ANY_ASSIGNEE) return Boolean(assignee && assignee.kind);
  return assigneeKey(assignee) === want;
};

const matches = (issue, filters) => {
  if (filters.state && issue.state !== filters.state) return false;
  if (filters.status && issue.status !== filters.status) return false;
  if (filters.label && !(issue.labels || []).includes(filters.label)) return false;
  return assigneeMatches(issue.assignee, filters.assignee);
};

/** The same narrowing, applied here — what the cached list is painted through
 *  until the bridge's own answer lands. */
export const filterIssues = (issues, filters = {}) => (issues || []).filter((issue) => matches(issue, filters));

/**
 * Whether the reader has narrowed anything BEYOND the default.
 *
 * Which is what tells an empty list "nothing here yet" from "nothing matches
 * what you asked for", and what decides whether the Clear press is offered at
 * all. Measured against the default rather than against nothing: with Open as
 * the starting point (#33) every list would otherwise read as narrowed, the
 * Clear press would never go away, and a project with no issues in it would
 * be reported as a filter that matched none.
 */
export const filtersAreSet = (filters = {}) =>
  Object.keys(DEFAULT_FILTERS).some((name) => (filters[name] || "") !== DEFAULT_FILTERS[name]);

/**
 * Whether this read asks the bridge for less than the project's whole list.
 *
 * A different question from `filtersAreSet`, and the two stopped agreeing when
 * Open became the default (#33): the default narrows the READ — it asks for
 * open issues only — while narrowing nothing the READER chose.
 *
 * What depends on it is where the filter menus come from. They are built from
 * the whole list so that choosing a label never empties the menu it was chosen
 * from, and so that switching to Closed can still offer the labels only closed
 * issues wear. A read that narrowed anything is not that list, and the cache's
 * copy stands in for it.
 */
export const narrowsTheRead = (filters = {}) =>
  Boolean(filters.state || filters.status || filters.assignee || filters.label);

/** Newest first, by number descending — the order `issues.list` answers in, so
 *  an issue inserted optimistically lands where the next read will put it. */
export const sortIssues = (issues) =>
  [...(issues || [])].sort((left, right) => Number(right.number || 0) - Number(left.number || 0));

/** Every label any of these issues wears, alphabetically — the label filter's
 *  offer. Built from the project's whole list rather than from the narrowed
 *  one, so choosing a label never empties the menu it was chosen from.
 *
 *  A blank label is dropped: it is nothing a reader can ask for, and the empty
 *  string is already spoken for by "Any label" — two offers with one value is
 *  a menu whose second entry can never be reached. */
export const labelsOf = (issues) =>
  [...new Set((issues || []).flatMap((issue) => issue.labels || []).filter(Boolean))].sort();

/** Every assignee any of these issues has, as filter values. The caller names
 *  them (core/trackerAssignee.js knows the project's agents); this only says
 *  which ones are worth offering. */
export const assigneesOf = (issues) => {
  const seen = new Map();
  for (const issue of issues || []) {
    if (!issue.assignee || !issue.assignee.kind) continue;
    seen.set(assigneeKey(issue.assignee), issue.assignee);
  }
  return [...seen.entries()].map(([value, assignee]) => ({ value, assignee }));
};

/**
 * What the filter bar offers, one list per control.
 *
 * `states` and `statuses` are what the vocabulary holds — every column is
 * offered whether or not an issue stands in it, because a filter is a question
 * about the project and not a summary of it. `labels` and `assignees` are what
 * the project's issues actually carry: there is no closed set to offer.
 */
export function filterOptions(issues, columns, labelFor = () => "") {
  return {
    states: [
      { value: "", label: "Open and closed" },
      { value: "open", label: "Open" },
      { value: "closed", label: "Closed" },
    ],
    statuses: [{ value: "", label: "Any column" }, ...columnsOf(columns).map((column) => ({ value: column.id, label: column.name }))],
    assignees: [
      { value: "", label: "Anyone" },
      { value: ANY_ASSIGNEE, label: "Anyone assigned" },
      { value: UNASSIGNED, label: "Unassigned" },
      ...assigneesOf(issues).map(({ value, assignee }) => ({ value, label: labelFor(assignee) || value })),
    ],
    labels: [{ value: "", label: "Any label" }, ...labelsOf(issues).map((label) => ({ value: label, label }))],
  };
}
