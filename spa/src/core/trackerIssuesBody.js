// The Issues tab's body: the one part of the tab that repaints.
//
// Everything above it — the header, the four filters, the Clear press — is
// mounted once and never redrawn (core/trackerPaneChrome.js). What is left is
// this, and it is painted the way the timeline is: by KEY
// (core/patchList.js), so a feed move or a push changes only the rows that
// changed. A row that is still there is still the same element afterwards,
// which is what keeps the press on it pressable, the keyboard where it was,
// and the browser's scroll anchoring holding a node it can still find.
//
// The frame around the rows — the empty line, the board's columns and their
// counts — is patched too (core/domPatch.js), and the lists inside it wear
// `data-keyed-list` so that patch stops at their door: their children belong
// to patchList, and two painters writing one list is one of them undoing the
// other.
//
// Wiring is handed in rather than done here, and it runs once per element that
// had to be MADE. A handler attached on a patch is a handler attached twice.

import { KEYED_LIST_ATTRIBUTE, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { emptyListHtml, issueRowHtml } from "./trackerListRender.js";
import { boardFrameHtml, issueCardHtml } from "./trackerBoardRender.js";
import { esc } from "./text.js";

const keyOf = (issue) => issue.id;

/**
 * The list's frame.
 *
 * The `<ul>` leads and is always there, empty or not: an empty list that still
 * holds its list is one the next row is INSERTED into rather than one that has
 * to be built. The empty line follows it rather than replacing it — a zero-row
 * list takes no height, so the reader sees the same thing either way, and the
 * patch above never has to tell a `<div>` apart from a `<ul>` in the same slot.
 */
const listFrameHtml = (issues, filters, paging) =>
  `<ul class="issue-rows" ${KEYED_LIST_ATTRIBUTE}></ul>${issues.length ? "" : emptyListHtml(filters || {})}` +
  (paging?.total > 0
    ? `<div class="issue-paging"><span>Showing ${issues.length} of ${paging.total}</span>${issues.length < paging.total
      ? '<button class="btn mini" type="button" data-issue-more>Load more issues</button>' : ""}</div>`
    : "");

/** The list view: one keyed row per issue. */
export function paintIssueRows(body, issues, context, wire) {
  patchInnerHtml(body, listFrameHtml(issues, context.filters, context.paging));
  patchList(body.querySelector(".issue-rows"), issues, {
    keyOf,
    render: (issue) => issueRowHtml(issue, context),
    wire,
  });
  const more = body.querySelector("[data-issue-more]");
  if (more) more.onclick = context.paging.more;
}

/** Groups keep one keyed list each. Collapsing hides rows without discarding
 *  their cached records or changing the order of the next page. */
export function paintGroupedIssueRows(body, groups, context, wire) {
  const loaded = groups.reduce((count, group) => count + group.issues.length, 0);
  const sections = groups.map((group) => `<section class="issue-group" data-issue-group="${group.id}">
    <button class="issue-group-heading" type="button" data-issue-group-toggle="${group.id}" aria-expanded="${!group.collapsed}">
      <span>${esc(group.title)}</span><span class="issue-group-count">${group.count}</span>
    </button>
    <ul class="issue-rows" ${KEYED_LIST_ATTRIBUTE}${group.collapsed ? " hidden" : ""}></ul>
  </section>`).join("");
  const pager = context.paging?.total > 0
    ? `<div class="issue-paging"><span>Showing ${loaded} of ${context.paging.total}</span>${loaded < context.paging.total
      ? '<button class="btn mini" type="button" data-issue-more>Load more issues</button>' : ""}</div>` : "";
  patchInnerHtml(body, sections + (context.paging?.total ? "" : emptyListHtml(context.filters || {})) + pager);
  for (const group of groups) {
    const section = [...body.querySelectorAll("[data-issue-group]")]
      .find((element) => element.dataset.issueGroup === group.id);
    patchList(section.querySelector(".issue-rows"), group.issues, {
      keyOf,
      render: (issue) => issueRowHtml(issue, context),
      wire,
    });
  }
  body.querySelectorAll("[data-issue-group-toggle]").forEach((button) => {
    button.onclick = () => context.onToggleGroup(button.dataset.issueGroupToggle);
  });
  const more = body.querySelector("[data-issue-more]");
  if (more) more.onclick = context.paging.more;
}

/** Each column's card list, found by the column it drops into rather than by a
 *  selector built out of one: an id is data, and a selector is a language. */
const cardListsIn = (body) =>
  new Map([...body.querySelectorAll("[data-column-drop]")].map((cards) => [cards.dataset.columnDrop, cards]));

/** The board: the columns and their counts are patched, the cards in them are
 *  keyed. A card moved by a drag or an arrow key costs the two columns it
 *  moved between and nothing else on the board. */
export function paintIssueBoard(body, board, context, wire) {
  patchInnerHtml(body, boardFrameHtml(board, context));
  const cardLists = cardListsIn(body);
  for (const column of board) {
    const cards = cardLists.get(column.id);
    if (!cards) continue;
    patchList(cards, column.issues, { keyOf, render: (issue) => issueCardHtml(issue, context), wire });
  }
}
