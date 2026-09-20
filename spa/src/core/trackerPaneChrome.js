// The Issues tab's chrome: the header and the filter bar, mounted ONCE.
//
// Zech, #43: "The inputs/selects really shouldn't be redrawing ever." They do
// not. Every control here is made when the tab mounts and is the same DOM node
// for the life of the pane — through a push, a feed move, a re-read, a view
// switch and a filter change. Nothing in a paint can take the reader's focus,
// their caret, or the menu they have open, because a paint never touches the
// control at all; it only ever says three things to one that is already there:
//
//   • which options it offers — a newly filed label or a newly-seen agent is an
//     inserted row inside the menu, and every other row is the row it was;
//   • what is chosen, which is written from the state and never read back out
//     of the control;
//   • whether the Clear press is worth offering — hidden rather than removed,
//     because removing a button is removing whatever focus was on it.
//
// The body below the bar is the caller's: this hands it back empty and never
// writes into it again (core/trackerIssuesBody.js paints it, by key).
//
// #44 put custom dropdowns where the native selects were, and the contract did
// not move: made once, told what is on offer and what is chosen, never
// re-created. What a menu holds beyond that — whether it is open, the query
// half typed into it, the row the arrow keys have walked to — is the reader's,
// and `update` does not touch any of it (core/filterMenuControl.js).

import { ICON_PLUS } from "./icons.js";
import { chosenOf, filtersAreSet } from "./trackerFilters.js";
import { mountFilterMenu } from "./filterMenuControl.js";

export const LIST_VIEW = "list";
export const BOARD_VIEW = "board";

const VIEWS = [
  { id: LIST_VIEW, label: "List" },
  { id: BOARD_VIEW, label: "Board" },
];

/**
 * The four filters, in the order they are read.
 *
 * Each names the filter it writes, what it is called, which of
 * `filterOptions`' lists it offers, and whether it takes one answer or several.
 *
 * State and Column take one: they are questions with mutually exclusive
 * answers, and a board narrowed to two columns is not a board. They use the
 * same control in single-select mode all the same, so the bar reads as one
 * family rather than as two native selects beside two of ours.
 *
 * Labels and Assignee take several, any-of. A selection of several is said
 * differently for each (`summary`): label names are short and interchangeable,
 * so the filter names itself and counts them; an assignee is somebody, and the
 * first one is the most useful word on the bar.
 */
const FILTERS = [
  { name: "state", label: "State", offer: (options) => options.states },
  { name: "status", label: "Column", offer: (options) => options.statuses },
  { name: "assignee", label: "Assignee", multi: true, summary: "first", offer: (options) => options.assignees },
  { name: "label", label: "Labels", multi: true, summary: "count", offer: (options) => options.labels },
];

const viewButtonHtml = (view) =>
  `<button class="btn mini issue-view" type="button" data-issue-view="${view.id}">${view.label}</button>`;

const chromeHtml = () => `<div class="issue-head">
    <div class="issue-views" role="group" aria-label="How to lay the issues out">${VIEWS.map(viewButtonHtml).join("")}</div>
    <button class="btn mini primary issue-new" type="button" data-issue-new>${ICON_PLUS}<span>New issue</span></button>
  </div>
  <div class="issue-filters" role="group" aria-label="Filter issues">
    <button class="btn mini" type="button" data-issue-filter-clear hidden>Clear</button>
  </div>
  <div class="issue-compose-slot"></div>
  <div class="issue-body"></div>`;

const showView = (button, view) => {
  const active = button.dataset.issueView === view;
  button.classList.toggle("active", active);
  button.setAttribute("aria-pressed", String(active));
};

/**
 * Mount the header and the filter bar into `host`, once.
 *
 * Hands back the body they sit above, the slot the inline composer opens into
 * (#57), and an `update` that makes them say what the state says. The four
 * callbacks are the only way anything leaves here:
 * `onFilter` is given the filter's name and its new value, and the rest take
 * nothing — a press is a press.
 */
export function mountIssuesChrome(host, { onView, onNew, onFilter, onClear }) {
  host.innerHTML = chromeHtml();
  const body = host.querySelector(".issue-body");
  // Between the bar and the rows, and OUTSIDE the body: the body is repainted
  // whenever a push says an issue moved, and a composer somebody is typing
  // into is not something a push gets to take away (#57).
  const composeSlot = host.querySelector(".issue-compose-slot");
  const viewButtons = [...host.querySelectorAll("[data-issue-view]")];
  const bar = host.querySelector(".issue-filters");
  const clear = host.querySelector("[data-issue-filter-clear]");

  // The menus are mounted ahead of the Clear press, which the frame already
  // holds: a bar built once is a bar whose order is the markup's, not the
  // order the mounts happened to run in.
  const menus = FILTERS.map((filter) => ({
    ...filter,
    control: mountFilterMenu(bar, { ...filter, onChange: (chosen) => onFilter(filter.name, chosen) }),
  }));
  menus.forEach(({ control }) => bar.insertBefore(control.element, clear));

  viewButtons.forEach((button) => {
    button.onclick = () => onView(button.dataset.issueView);
  });
  host.querySelector("[data-issue-new]").onclick = onNew;
  clear.onclick = () => {
    menus.forEach(({ control }) => control.close());
    onClear();
  };

  return {
    body,
    composeSlot,
    update({ view, options, filters }) {
      viewButtons.forEach((button) => showView(button, view));
      menus.forEach(({ offer, name, control }) => control.update(offer(options), chosenOf(filters[name])));
      clear.hidden = !filtersAreSet(filters);
    },
    dispose() {
      menus.forEach(({ control }) => control.dispose());
    },
  };
}
