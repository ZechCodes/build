// The two-column layout's list column on a narrow viewport. Above the stacking
// width Changes and Files are a list column beside a detail column; below it
// there is only room for one of the two, so the list becomes a drawer — it
// floats over the left of the pane on a scrim, and a handle on the pane's edge
// pulls it out and pushes it back.
//
// This is the primitive's, not each pane's: both panes hand it the same two
// things (the column to float, and which of its rows fill the detail), and the
// geometry lives in one .pane-split rule. The handle and the scrim are in the
// markup at every width — CSS is what decides they only paint where there is a
// drawer to work.
//
// DOM-light, like the panes it serves: it toggles one class and keeps the
// handle's accessible state in step.

import { esc } from "./text.js";

const OPEN_CLASS = "drawer-open";

let listSeq = 0;

/** Pure: the split's own two extra children — the scrim an open drawer sits on,
 *  and the handle that moves it. `label` names the column being moved ("commits",
 *  "files"), which is what the handle announces. */
export function paneDrawerHtml(label) {
  return (
    `<div class="pane-scrim" data-pane-scrim="1"></div>` +
    `<button class="pane-handle" type="button" data-pane-handle="1" aria-expanded="false" aria-label="Show ${esc(label)}" data-pane-label="${esc(label)}"></button>`
  );
}

/**
 * initPaneDrawer(split, { list, closeOnSelect }) — wire the drawer on a mounted
 * `.pane-split`. The handle toggles it; the scrim and Escape close it; so does a
 * click on a row inside `list` matching the `closeOnSelect` selector, because
 * picking one is what the drawer was opened for and what it picks is behind it.
 * A row that only moves the list (a directory) is not in that selector and
 * leaves the drawer where it is.
 *
 * Returns { open, close, isOpen, dispose } — dispose drops the document
 * listener, so a pane that remounts never leaves one behind.
 */
// eslint-disable-next-line complexity -- ratchet: initPaneDrawer is at 11, cap 10 — reduce it, then drop this line
export function initPaneDrawer(split, { list, closeOnSelect }) {
  const handle = split.querySelector("[data-pane-handle]");
  const scrim = split.querySelector("[data-pane-scrim]");
  const label = handle ? handle.dataset.paneLabel || "" : "";
  if (list && !list.id) list.id = `pane-list-${++listSeq}`;
  if (handle && list) handle.setAttribute("aria-controls", list.id);

  const isOpen = () => split.classList.contains(OPEN_CLASS);
  const setOpen = (open) => {
    split.classList.toggle(OPEN_CLASS, open);
    if (!handle) return;
    handle.setAttribute("aria-expanded", open ? "true" : "false");
    handle.setAttribute("aria-label", `${open ? "Hide" : "Show"} ${label}`);
  };
  const close = () => setOpen(false);

  if (handle) handle.addEventListener("click", () => setOpen(!isOpen()));
  if (scrim) scrim.addEventListener("click", close);
  if (list && closeOnSelect) {
    list.addEventListener("click", (event) => {
      if (event.target.closest(closeOnSelect)) close();
    });
  }
  // Escape is the whole-page gesture for "put this overlay away", so it is
  // heard wherever focus happens to be — the same reach the project rail's
  // scrim has.
  const onKeyDown = (event) => {
    if (event.key === "Escape" && isOpen()) close();
  };
  document.addEventListener("keydown", onKeyDown);

  return {
    open: () => setOpen(true),
    close,
    isOpen,
    dispose() {
      document.removeEventListener("keydown", onKeyDown);
    },
  };
}
