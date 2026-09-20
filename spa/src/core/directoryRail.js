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
// rather than a label under the glyph. The set is a tablist: one tab stop for
// the whole rail, the arrows walking within it, which is what a rail of two
// controls owes the keyboard.

import { ICON_CIRCLE_DOT, ICON_FOLDER, ICON_GIT_GRAPH } from "./icons.js";
import { changesTabLabel, esc, filesTabLabel, issuesTabLabel } from "./text.js";

/** The rail's faces, in reading order: what moved, then what is there. */
export const DIRECTORY_TABS = [
  { id: "changes", label: changesTabLabel, icon: ICON_GIT_GRAPH },
  { id: "files", label: filesTabLabel, icon: ICON_FOLDER },
];

/** A WORKSPACE's faces: the checkout's two, and the issues its agents hold
 *  (#29). Here rather than in the workspace view because this rail is the
 *  shell's column — it stands at every width, which is what makes the issues
 *  tab reachable on a phone, where a toolbar overlay was not.
 *
 *  A branch keeps `DIRECTORY_TABS` alone: it is a checkout with no agents
 *  standing in it, so there are no issues that are ITS issues. */
export const WORKSPACE_TABS = [...DIRECTORY_TABS, { id: "issues", label: issuesTabLabel, icon: ICON_CIRCLE_DOT }];

/** Where an arrow takes the highlight, as steps along the rail. Home and End
 *  are the same question asked absolutely, so they answer from one table too. */
const KEY_STEPS = { ArrowUp: -1, ArrowDown: 1 };
const KEY_ENDS = { Home: 0, End: -1 };

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

/**
 * paintDirectoryRail(host, { tabs, active, onSelect }) — draw the rail into the
 * shell's column and wire it. Idempotent: every paint rewrites the row and its
 * handlers, so a surface repaints by calling it again and there is nothing to
 * dispose. A directory with no git has one face, and passes the one tab.
 *
 * Automatic activation, the way a tablist of two behaves: an arrow both moves
 * the focus and opens what it lands on.
 */
export function paintDirectoryRail(host, { tabs = DIRECTORY_TABS, active, onSelect }) {
  host.setAttribute("role", "tablist");
  host.setAttribute("aria-orientation", "vertical");
  // A paint answering a press rewrites the cell the keyboard stands on; the
  // keyboard is handed the active cell that replaces it, or it lands on nothing.
  const keyboardHere = host.contains(document.activeElement);
  host.innerHTML = directoryRailHtml(tabs, active);
  if (keyboardHere) host.querySelector("[aria-selected='true']")?.focus();
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
