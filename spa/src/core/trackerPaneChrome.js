// The Issues tab's chrome: the header and the filter bar, mounted ONCE.
//
// Zech, #43: "The inputs/selects really shouldn't be redrawing ever." They do
// not. Every control here is made when the tab mounts and is the same DOM node
// for the life of the pane — through a push, a feed move, a re-read, a view
// switch and a filter change. Nothing in a paint can take the reader's focus,
// their caret, or the menu they have open, because a paint never touches the
// control at all; it only ever says three things to one that is already there:
//
//   • which options it offers — patched in place by value (core/patchList.js),
//     so a new label or a newly-seen assignee is an inserted `<option>` and
//     every other option is the node it already was;
//   • what it is set to — written only when it disagrees with the state, which
//     is never for the control the change came from;
//   • whether the Clear press is worth offering — hidden rather than removed,
//     because removing a button is removing whatever focus was on it.
//
// The body below the bar is the caller's: this hands it back empty and never
// writes into it again (core/trackerIssuesBody.js paints it, by key).
//
// #44 puts custom dropdowns where the native selects are. It is the same
// contract — made once, patched, never re-created — so the shape survives it.

import { ICON_PLUS } from "./icons.js";
import { esc } from "./text.js";
import { patchList } from "./patchList.js";
import { filtersAreSet } from "./trackerFilters.js";

export const LIST_VIEW = "list";
export const BOARD_VIEW = "board";

const VIEWS = [
  { id: LIST_VIEW, label: "List" },
  { id: BOARD_VIEW, label: "Board" },
];

/** The four filters, in the order they are read. Each names the filter it
 *  writes, what it is called where it cannot be seen — the bar has no room for
 *  four visible labels — and which of `filterOptions`' lists it offers. */
const FILTERS = [
  { name: "state", label: "State", offer: (options) => options.states },
  { name: "status", label: "Column", offer: (options) => options.statuses },
  { name: "assignee", label: "Assignee", offer: (options) => options.assignees },
  { name: "label", label: "Label", offer: (options) => options.labels },
];

const viewButtonHtml = (view) =>
  `<button class="btn mini issue-view" type="button" data-issue-view="${view.id}">${view.label}</button>`;

/** A filter, with no options in it. They arrive on the first update and are
 *  patched by value from then on; the select itself never comes back. */
const selectHtml = (filter) =>
  `<select class="issue-filter" data-issue-filter="${filter.name}" aria-label="${filter.label}"></select>`;

const chromeHtml = () => `<div class="issue-head">
    <div class="issue-views" role="group" aria-label="How to lay the issues out">${VIEWS.map(viewButtonHtml).join("")}</div>
    <button class="btn mini primary issue-new" type="button" data-issue-new>${ICON_PLUS}<span>New issue</span></button>
  </div>
  <div class="issue-filters" role="group" aria-label="Filter issues">
    ${FILTERS.map(selectHtml).join("")}
    <button class="btn mini" type="button" data-issue-filter-clear hidden>Clear</button>
  </div>
  <div class="issue-body"></div>`;

/** One option, keyed by the value it stands for. No `selected` attribute: what
 *  a select is set to is said once, below, against the state — an attribute
 *  written on every paint would fight the reader for the control. */
const optionHtml = (option) => `<option value="${esc(option.value)}">${esc(option.label)}</option>`;

const offerOptions = (select, options) =>
  patchList(select, options, { keyOf: (option) => option.value, render: optionHtml });

/** Set only when it disagrees. A select written to the value it already holds
 *  is a select whose open menu closes under the reader. */
const showValue = (select, value) => {
  const wanted = value || "";
  if (select.value !== wanted) select.value = wanted;
};

const showView = (button, view) => {
  const active = button.dataset.issueView === view;
  button.classList.toggle("active", active);
  button.setAttribute("aria-pressed", String(active));
};

/**
 * Mount the header and the filter bar into `host`, once.
 *
 * Hands back the body they sit above and an `update` that makes them say what
 * the state says. The four callbacks are the only way anything leaves here:
 * `onFilter` is given the filter's name and its new value, and the rest take
 * nothing — a press is a press.
 */
export function mountIssuesChrome(host, { onView, onNew, onFilter, onClear }) {
  host.innerHTML = chromeHtml();
  const body = host.querySelector(".issue-body");
  const viewButtons = [...host.querySelectorAll("[data-issue-view]")];
  const selects = FILTERS.map((filter) => ({
    ...filter,
    control: host.querySelector(`[data-issue-filter="${filter.name}"]`),
  }));
  const clear = host.querySelector("[data-issue-filter-clear]");

  viewButtons.forEach((button) => {
    button.onclick = () => onView(button.dataset.issueView);
  });
  host.querySelector("[data-issue-new]").onclick = onNew;
  selects.forEach(({ name, control }) => {
    control.onchange = () => onFilter(name, control.value);
  });
  clear.onclick = onClear;

  return {
    body,
    update({ view, options, filters }) {
      viewButtons.forEach((button) => showView(button, view));
      selects.forEach(({ offer, name, control }) => {
        offerOptions(control, offer(options));
        showValue(control, filters[name]);
      });
      clear.hidden = !filtersAreSet(filters);
    },
  };
}
