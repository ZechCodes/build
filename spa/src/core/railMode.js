// Which face the inbox rail is showing — the one list across every project, or
// the projects with their work beneath them — and which project blocks the user
// has folded shut. Pure functions over an injected Web-Storage-shaped object
// ({getItem, setItem}) so the module is unit-testable under node; the app
// passes localStorage. Both choices are conveniences, never fatal — a browser
// that refuses storage still gets the default rail.

import { esc } from "./text.js";
import { ICON_FOLDERS, ICON_INBOX } from "./icons.js";

export const RAIL_VIEW_KEY = "build.inbox.view";
export const FOLDED_PROJECTS_KEY = "build.inbox.folded";

/** The two faces, in the order the switch offers them. */
export const RAIL_VIEWS = ["inbox", "projects"];

const DEFAULT_RAIL_VIEW = "inbox";

/** The remembered face. Anything missing, unrecognised, or unreadable is the
 *  inbox — the one you had before you ever chose. */
export function loadRailView(storage) {
  try {
    const stored = storage.getItem(RAIL_VIEW_KEY);
    return RAIL_VIEWS.includes(stored) ? stored : DEFAULT_RAIL_VIEW;
  } catch {
    return DEFAULT_RAIL_VIEW;
  }
}

/** Remember a face. One that does not exist is not written, so a bad value can
 *  never be read back later. */
export function persistRailView(view, storage) {
  if (!RAIL_VIEWS.includes(view)) return;
  try {
    storage.setItem(RAIL_VIEW_KEY, view);
  } catch {
    /* storage disabled — the rail still works, it just forgets */
  }
}

/** What the user has said of each project's fold: project key → folded (the
 *  account-wide name, core/deviceKey.js). A project they have said nothing
 *  about is absent, and the face decides for it. Anything stored that is not
 *  that shape is nothing said, and so is anything an older client wrote under
 *  a bare project id — it names no project now, so no block ever asks for it. */
export function loadProjectFolds(storage) {
  try {
    const parsed = JSON.parse(storage.getItem(FOLDED_PROJECTS_KEY) || "{}");
    if (Array.isArray(parsed)) {
      return new Map(parsed.filter((id) => typeof id === "string").map((id) => [id, true]));
    }
    if (!parsed || typeof parsed !== "object") return new Map();
    return new Map(Object.entries(parsed).filter(([, folded]) => typeof folded === "boolean"));
  } catch {
    return new Map();
  }
}

export function persistProjectFolds(folds, storage) {
  try {
    storage.setItem(FOLDED_PROJECTS_KEY, JSON.stringify(Object.fromEntries(folds)));
  } catch {
    /* storage disabled — the folds last the session */
  }
}

const VIEW_SWITCH = [
  { view: "inbox", label: "Inbox", icon: ICON_INBOX },
  { view: "projects", label: "Projects", icon: ICON_FOLDERS },
];

/** The switch at the head's right edge: one icon per face, the standing one
 *  pressed. The icons are build-time constants, never user data. */
export function railViewSwitchHtml(view) {
  return VIEW_SWITCH.map(
    ({ view: face, label, icon }) =>
      `<button class="iconbtn inbox-view${face === view ? " on" : ""}" type="button" data-inbox-view="${face}" title="${esc(label)}" aria-label="${esc(label)}" aria-pressed="${face === view ? "true" : "false"}">${icon}</button>`,
  ).join("");
}
