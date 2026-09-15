// The two-column layout's list column on a narrow viewport. Above the stacking
// width Changes and Files are a list column beside a detail column; below it
// there is only room for one of the two, so the list becomes a drawer — it
// drops in from the TOP of the pane over the detail, on a scrim, and a trigger
// row across the head of the pane drops it and puts it back.
//
// The trigger is a row rather than a tab on the pane's edge because it has
// something to say: with the rail behind it, the line that told you where you
// were standing is behind it too. So the trigger carries that line — the open
// commit, the open file — and reads as the dropdown it is.
//
// This is the primitive's, not each pane's: both panes hand it the same three
// things (the column to drop, which of its rows fill the detail, and the words
// for what the detail is showing), and the geometry lives in one .pane-split
// rule. The trigger and the scrim are in the markup at every width — CSS is
// what decides they only paint where there is a drawer to work.
//
// DOM-light, like the panes it serves: it toggles one class, writes one line of
// text, and keeps the trigger's accessible state in step.

import { esc } from "./text.js";

const OPEN_CLASS = "drawer-open";

let listSeq = 0;

/** Pure: the split's own two extra children — the scrim an open drawer sits on,
 *  and the trigger row that drops it. `label` names the column being moved
 *  ("commits", "files"), which is what the trigger announces and what it says
 *  when the pane offers no words of its own. */
export function paneDrawerHtml(label) {
  return (
    `<div class="pane-scrim" data-pane-scrim="1"></div>` +
    `<button class="pane-handle" type="button" data-pane-handle="1" aria-expanded="false" aria-label="Show ${esc(label)}" data-pane-label="${esc(label)}">` +
    `<span class="pane-handle-what" data-pane-summary="1"></span>` +
    `<span class="pane-handle-caret" aria-hidden="true"></span>` +
    `</button>`
  );
}

/** The one line the trigger carries: what the detail column is showing. */
const paintSummary = (handle, words) => {
  handle.querySelector("[data-pane-summary]").textContent = words;
};

/**
 * initPaneDrawer(split, { list, closeOnSelect, summary }) — wire the drawer on a
 * mounted `.pane-split`. The trigger toggles it; the scrim and Escape close it;
 * so does a click on a row inside `list` matching the `closeOnSelect` selector,
 * because picking one is what the drawer was opened for and what it picks is
 * behind it. A row that only moves the list (a directory) is not in that
 * selector and leaves the drawer where it is.
 *
 * `summary()` returns the words for what the detail column is showing. The pane
 * owns that selection, so the PANE says when it moved, by calling `refresh()`
 * from its own render — a click is heard here before the pane has acted on it.
 *
 * Returns { open, close, isOpen, refresh, dispose } — dispose drops the document
 * listener, so a pane that remounts never leaves one behind.
 */
export function initPaneDrawer(split, { list, closeOnSelect, summary }) {
  // Every part this reaches for is part this module wrote (paneDrawerHtml), or
  // a column the calling pane always has: there is nothing here to guard
  // against, and guarding anyway is what put this function over the cap.
  const handle = split.querySelector("[data-pane-handle]");
  const scrim = split.querySelector("[data-pane-scrim]");
  const label = handle.dataset.paneLabel;
  if (!list.id) list.id = `pane-list-${++listSeq}`;
  handle.setAttribute("aria-controls", list.id);

  const words = summary || (() => label);
  const refresh = () => paintSummary(handle, words());

  const isOpen = () => split.classList.contains(OPEN_CLASS);
  const setOpen = (open) => {
    split.classList.toggle(OPEN_CLASS, open);
    handle.setAttribute("aria-expanded", open ? "true" : "false");
    handle.setAttribute("aria-label", `${open ? "Hide" : "Show"} ${label}`);
  };
  const close = () => setOpen(false);

  handle.addEventListener("click", () => setOpen(!isOpen()));
  scrim.addEventListener("click", close);
  list.addEventListener("click", (event) => {
    if (event.target.closest(closeOnSelect)) close();
  });
  // Escape is the whole-page gesture for "put this overlay away", so it is
  // heard wherever focus happens to be — the same reach the project rail's
  // scrim has.
  const onKeyDown = (event) => {
    if (event.key === "Escape" && isOpen()) close();
  };
  document.addEventListener("keydown", onKeyDown);
  refresh();

  return {
    open: () => setOpen(true),
    close,
    isOpen,
    refresh,
    dispose() {
      document.removeEventListener("keydown", onKeyDown);
    },
  };
}
