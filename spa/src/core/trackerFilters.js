// The Tasks tab's filters: state, status, assignee and label.
//
// They match `tasks.list`'s params one for one, so a filter is a PARAM and not
// a client-side pass over everything — the bridge answers the narrowed list.
// The local pass below is for the other half of the same act: the cache holds
// the project's whole list (core/trackerCache.js), so the first paint applies
// the same filters to what is already on disk while the narrowed read is still
// in flight. Two readings of one rule, so they are written once here.
//
// Since #44 a filter holds a LIST. `tasks.list` still takes one label and one
// assignee, so a selection of several is narrowed here, over the whole list the
// cache already holds — exactly, not approximately, because that list is the
// project's whole list and the sync layer keeps it so. The param goes out only
// when exactly one thing is chosen, which keeps a one-of narrowing a real read
// rather than a read plus a pass.
//
// Every reader below takes a list OR the single string filters were before it,
// so a caller that has not grown a menu yet is not a caller that broke.
//
// No DOM, no app imports.

import { UNASSIGNED, assigneeFromKey, assigneeKey, columnsOf } from "./trackerModel.js";

/**
 * What one filter holds, always as a list.
 *
 * A menu answers a list; the default and every older caller answer a string;
 * "nothing chosen" is the empty list, the empty string and absent alike. One
 * reading of all of them, here, so nothing downstream has to ask.
 */
export const chosenOf = (value) =>
  Array.isArray(value) ? value.filter((one) => one !== "" && one != null) : value ? [String(value)] : [];

/** Whether two selections are the same selection. */
const sameChoice = (left, right) =>
  left.length === right.length && left.every((value, index) => value === right[index]);

/** The one value a param can carry, or nothing. `tasks.list` narrows on ONE
 *  label and ONE assignee, so two chosen is not a narrower read — it is a
 *  wider one, narrowed here afterwards. */
const onlyOne = (value) => {
  const chosen = chosenOf(value);
  return chosen.length === 1 ? chosen[0] : undefined;
};

/** Nothing narrowed: every task of the project, open and closed. What the
 *  WHOLE list is read with — the menus are built from it, and a menu offering
 *  only what the current narrowing left could not offer Closed at all. */
export const NO_FILTERS = Object.freeze({ state: "", status: "", assignee: "", label: "" });

/**
 * What the tab opens on, and what Clear goes back to.
 *
 * The maintainer, deciding the question #28 raised: "If closed means done we
 * shouldn't show them in the default view." So Open is the default and closed
 * tasks are a thing you ask for — the filter still offers Closed and Open and
 * closed, and a closed row wears its own chip when one is on screen
 * (core/trackerListRender.js), so it can never be misread as open.
 *
 * Not a narrowing of NO_FILTERS but a starting point beside it: "is this list
 * narrowed" is a question about what the reader asked for beyond the default,
 * which is what `filtersAreSet` answers below.
 */
export const DEFAULT_FILTERS = Object.freeze({ ...NO_FILTERS, state: "open" });

/** The word that means "assigned to somebody, no matter who". `tasks.list`
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

/** The filters as `tasks.list` params. A filter nobody set is left off rather
 *  than sent empty: the verb reads an absent param as "do not narrow on this",
 *  and an empty string is not that. */
export function taskListParams(projectId, filters = {}) {
  const params = { project_id: projectId };
  const state = onlyOne(filters.state);
  const status = onlyOne(filters.status);
  const label = onlyOne(filters.label);
  if (state) params.state = state;
  if (status) params.status = status;
  if (label) params.label = label;
  const assignee = assigneeParam(onlyOne(filters.assignee));
  if (assignee !== undefined) params.assignee = assignee;
  return params;
}

const assigneeMatches = (assignee, want) =>
  want === ANY_ASSIGNEE ? Boolean(assignee && assignee.kind) : assigneeKey(assignee) === want;

/** Any of them, or none chosen at all. Any-of and not all-of: a reader ticking
 *  a second label is widening what they will accept, which is what a list of
 *  checkboxes reads as everywhere else. */
const anyOf = (value, wants) => {
  const chosen = chosenOf(value);
  return !chosen.length || chosen.some((want) => wants(want));
};

const matches = (task, filters) =>
  anyOf(filters.state, (want) => task.state === want) &&
  anyOf(filters.status, (want) => task.status === want) &&
  anyOf(filters.label, (want) => (task.labels || []).includes(want)) &&
  anyOf(filters.assignee, (want) => assigneeMatches(task.assignee, want));

/** The same narrowing, applied here — what the cached list is painted through
 *  until the bridge's own answer lands. */
export const filterTasks = (tasks, filters = {}) => (tasks || []).filter((task) => matches(task, filters));

/**
 * Whether the reader has narrowed anything BEYOND the default.
 *
 * Which is what tells an empty list "nothing here yet" from "nothing matches
 * what you asked for", and what decides whether the Clear press is offered at
 * all. Measured against the default rather than against nothing: with Open as
 * the starting point (#33) every list would otherwise read as narrowed, the
 * Clear press would never go away, and a project with no tasks in it would
 * be reported as a filter that matched none.
 */
export const filtersAreSet = (filters = {}, baseline = DEFAULT_FILTERS) =>
  Object.keys(baseline).some((name) => !sameChoice(chosenOf(filters[name]), chosenOf(baseline[name])));

/**
 * Whether this read asks the bridge for less than the project's whole list.
 *
 * A different question from `filtersAreSet`, and the two stopped agreeing when
 * Open became the default (#33): the default narrows the READ — it asks for
 * open tasks only — while narrowing nothing the READER chose.
 *
 * What depends on it is where the filter menus come from. They are built from
 * the whole list so that choosing a label never empties the menu it was chosen
 * from, and so that switching to Closed can still offer the labels only closed
 * tasks wear. A read that narrowed anything is not that list, and the cache's
 * copy stands in for it.
 */
export const narrowsTheRead = (filters = {}) =>
  Object.keys(NO_FILTERS).some((name) => chosenOf(filters[name]).length > 0);

/** Newest first, by number descending — the order `tasks.list` answers in, so
 *  a task inserted optimistically lands where the next read will put it. */
export const sortTasks = (tasks) =>
  [...(tasks || [])].sort((left, right) => Number(right.number || 0) - Number(left.number || 0));

/** Every label any of these tasks wears, alphabetically — the label filter's
 *  offer. Built from the project's whole list rather than from the narrowed
 *  one, so choosing a label never empties the menu it was chosen from.
 *
 *  A blank label is dropped: it is nothing a reader can ask for, and the empty
 *  string is already spoken for by "Any label" — two offers with one value is
 *  a menu whose second entry can never be reached. */
export const labelsOf = (tasks) =>
  [...new Set((tasks || []).flatMap((task) => task.labels || []).filter(Boolean))].sort();

/** Every assignee any of these tasks has, as filter values. The caller names
 *  them (core/trackerAssignee.js knows the project's agents); this only says
 *  which ones are worth offering. */
export const assigneesOf = (tasks) => {
  const seen = new Map();
  for (const task of tasks || []) {
    if (!task.assignee || !task.assignee.kind) continue;
    seen.set(assigneeKey(task.assignee), task.assignee);
  }
  return [...seen.entries()].map(([value, assignee]) => ({ value, assignee }));
};

/**
 * What the filter bar offers, one list per control.
 *
 * `states` and `statuses` are what the vocabulary holds — every column is
 * offered whether or not a task stands in it, because a filter is a question
 * about the project and not a summary of it. `labels` and `assignees` are what
 * the project's tasks actually carry: there is no closed set to offer.
 */
/**
 * Who the Tasks tab can narrow by.
 *
 * The three standing answers, then You, then every agent of the project
 * grouped by the workspace it stands on — the same grouping the assignee
 * picker uses (core/trackerAssignee.js), because one agent should be found the
 * same way wherever it is chosen. Then anybody a task actually names who is
 * none of those: an agent that has left the project still holds tasks, and a
 * filter that cannot name it cannot find them.
 *
 * `groups` is optional: a caller with no feed to hand still gets the standing
 * answers and everybody the tasks name.
 */
const STANDING_ASSIGNEES = Object.freeze([
  { value: "", label: "Anyone" },
  { value: ANY_ASSIGNEE, label: "Anyone assigned" },
  { value: UNASSIGNED, label: "Unassigned" },
  { value: "user", label: "You" },
]);

/** The project's agents, under the workspace each stands on. */
const agentAssignees = (groups) =>
  (groups || []).flatMap((group) =>
    (group.agents || []).map((agent) => ({ value: `agent:${agent.id}`, label: agent.label, group: group.name })),
  );

/** Anybody a task names who is none of the above. An agent that has left the
 *  project still holds tasks, and a filter that cannot name it cannot find
 *  them. */
const strandedAssignees = (tasks, labelFor) =>
  assigneesOf(tasks).map(({ value, assignee }) => ({
    value,
    label: labelFor(assignee) || value,
    group: "No longer in this project",
  }));

/** The first of each value and none of the repeats — an agent standing on a
 *  workspace is the same agent as the one holding a task. */
const firstOfEach = (rows) => {
  const named = new Set();
  return rows.filter((row) => !named.has(row.value) && named.add(row.value));
};

export function assigneeFilterOptions(tasks, labelFor = () => "", groups = []) {
  return firstOfEach([...STANDING_ASSIGNEES, ...agentAssignees(groups), ...strandedAssignees(tasks, labelFor)]);
}

export function filterOptions(tasks, columns, labelFor = () => "", groups = []) {
  return {
    states: [
      { value: "", label: "Open and closed" },
      { value: "open", label: "Open" },
      { value: "closed", label: "Closed" },
    ],
    statuses: [{ value: "", label: "Any column" }, ...columnsOf(columns).map((column) => ({ value: column.id, label: column.name }))],
    assignees: assigneeFilterOptions(tasks, labelFor, groups),
    labels: [{ value: "", label: "Any label" }, ...labelsOf(tasks).map((label) => ({ value: label, label }))],
  };
}
