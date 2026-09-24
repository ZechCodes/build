// A checkout's two faces — what moved (Changes) and what is there (Files) — as
// a rail of icons down the left edge of the surface.
//
// It is the SHELL's column (#dir-rail, between the inbox and the work), not a
// row inside the commit list or the file tree. That is the whole point of it:
// the tabs used to be painted into the list column of the pane they switched,
// so a phone — where that column is a drawer — put them at the bottom of a
// drawer, and every pane remount took them down with it. A column of the shell
// stands at every width, above nothing and inside nothing.
//
// One renderer, both surfaces: the workspace directory and the legacy branch
// checkout are the same two faces of one checkout, and a second copy of this
// rail is how the two would drift apart.
//
// Icon-only, so the words are the tooltip and the accessible name (core/text.js)
// rather than a label under the glyph. The faces are a tablist: one tab stop for
// all of them, the arrows walking within it, which is what a rail of two
// controls owes the keyboard.
//
// At the rail's foot, outside the tablist, is one more control: the sidebar
// toggle, which folds the list column beside the rail (the file tree, the
// commit rail) away so the detail takes the whole width. It is the rail's, not
// a face's, so switching faces keeps it; it is this browser's preference, kept
// in localStorage and never in the cache. The rail says which way it stands on
// its own host (data-sidebar), and one rule in styles.css reads that to drop
// the column — where the column is a drawer, that rule is not in force and the
// toggle is not drawn.

import { ICON_FOLDER, ICON_GIT_GRAPH, ICON_PANEL_LEFT_CLOSE, ICON_PANEL_LEFT_OPEN } from "./icons.js";
import { changesTabLabel, esc, filesTabLabel, sidebarToggleLabel } from "./text.js";

/** The rail's faces, in reading order: what moved, then what is there. */
export const DIRECTORY_TABS = [
  { id: "changes", label: changesTabLabel, icon: ICON_GIT_GRAPH },
  { id: "files", label: filesTabLabel, icon: ICON_FOLDER },
];

/** Where an arrow takes the highlight, as steps along the rail. Home and End
 *  are the same question asked absolutely, so they answer from one table too. */
const KEY_STEPS = { ArrowUp: -1, ArrowDown: 1 };
const KEY_ENDS = { Home: 0, End: -1 };

/** Where this browser keeps the sidebar toggle's choice: one key for every
 *  checkout, "true" for collapsed. Absent means expanded. */
export const SIDEBAR_COLLAPSED_KEY = "build.sidebarCollapsed";

function readSidebarCollapsed(storage) {
  try {
    return storage.getItem(SIDEBAR_COLLAPSED_KEY) === "true";
  } catch {
    return false; // a store that refuses to be read: the default, expanded
  }
}

function writeSidebarCollapsed(collapsed, storage) {
  try {
    storage.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed));
  } catch {
    /* a blocked or full store: the choice lasts until the rail repaints */
  }
}

/** Pure: one icon button per face. `icon` is markup and the only value here not
 *  escaped — an SVG constant from core/icons.js, resolved at build time and
 *  never user data. */
export function directoryRailHtml(tabs, active) {
  return tabs
    .map((tab) => {
      const selected = tab.id === active;
      return `<button class="dirtab${selected ? " active" : ""}" type="button" role="tab" data-tab="${esc(tab.id)}" title="${esc(tab.label)}" aria-label="${esc(tab.label)}" aria-selected="${selected}" tabindex="${selected ? 0 : -1}">${tab.icon}</button>`;
    })
    .join("");
}

/** The toggle's parts for a state: its words and its glyph, which show what a
 *  press does (fold the panel away, or bring it back). */
const sidebarToggleFace = (collapsed) => ({
  label: sidebarToggleLabel(collapsed),
  icon: collapsed ? ICON_PANEL_LEFT_OPEN : ICON_PANEL_LEFT_CLOSE,
});

/** Say the state on the host and on the toggle, in place: a press keeps the
 *  element the keyboard stands on. */
function paintSidebarState(host, toggle, collapsed) {
  const { label, icon } = sidebarToggleFace(collapsed);
  host.dataset.sidebar = collapsed ? "collapsed" : "expanded";
  toggle.title = label;
  toggle.setAttribute("aria-label", label);
  toggle.setAttribute("aria-expanded", String(!collapsed));
  toggle.innerHTML = icon;
}

/** Where the keyboard stood before a paint rewrote the rail, as the cell to
 *  hand it back to afterwards: the toggle, the open face, or nothing. */
function keyboardCellIn(host) {
  if (!host.contains(document.activeElement)) return null;
  return document.activeElement.matches("[data-sidebar-toggle]") ? "[data-sidebar-toggle]" : "[aria-selected='true']";
}

/**
 * paintDirectoryRail(host, { tabs, active, onSelect, storage }) — draw the rail
 * into the shell's column and wire it. Idempotent: every paint rewrites the row
 * and its handlers, so a surface repaints by calling it again and there is
 * nothing to dispose. A directory with no git has one face, and passes the one
 * tab. The sidebar toggle is painted from `storage` (localStorage) each time.
 *
 * Automatic activation, the way a tablist of two behaves: an arrow both moves
 * the focus and opens what it lands on.
 */
export function paintDirectoryRail(host, { tabs = DIRECTORY_TABS, active, onSelect, storage = globalThis.localStorage }) {
  // A paint answering a press rewrites the cell the keyboard stands on; the
  // keyboard is handed the cell that replaces it, or it lands on nothing.
  const keyboardCell = keyboardCellIn(host);
  // The column moves only on a press. A paint is arriving on a checkout or
  // switching its face, and the column is already where it stays.
  delete host.dataset.sidebarMotion;
  host.innerHTML =
    `<div class="dirtabs" role="tablist" aria-orientation="vertical">${directoryRailHtml(tabs, active)}</div>` +
    `<button class="dirtab dirtoggle" type="button" data-sidebar-toggle="1"></button>`;
  const toggle = host.querySelector("[data-sidebar-toggle]");
  paintSidebarState(host, toggle, readSidebarCollapsed(storage));
  if (keyboardCell) host.querySelector(keyboardCell)?.focus();
  toggle.onclick = () => {
    const collapsed = host.dataset.sidebar !== "collapsed";
    writeSidebarCollapsed(collapsed, storage);
    host.dataset.sidebarMotion = "press";
    paintSidebarState(host, toggle, collapsed);
  };
  wireFaces(host, onSelect);
}

/** The faces' presses and the arrow ring. The ring is the tab cells and
 *  nothing else: a key pressed anywhere else on the rail is the page's. */
function wireFaces(host, onSelect) {
  const cells = () => [...host.querySelectorAll("[data-tab]")];
  const openAt = (cell) => {
    cell.focus();
    onSelect(cell.dataset.tab);
  };
  host.querySelectorAll("[data-tab]").forEach((cell) => {
    cell.onclick = () => onSelect(cell.dataset.tab);
  });
  host.onkeydown = (event) => {
    const step = KEY_STEPS[event.key];
    const end = KEY_ENDS[event.key];
    if (step === undefined && end === undefined) return;
    const row = cells();
    const from = row.indexOf(event.target.closest("[data-tab]"));
    if (from < 0) return;
    event.preventDefault();
    openAt(step === undefined ? row.at(end) : row[(from + step + row.length) % row.length]);
  };
}
