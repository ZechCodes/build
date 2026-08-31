// The assignment overlay: the fields that say where an implementation is handed
// to, over the page instead of inside the issue rail.
//
// The rail is a list of stages. A form that unfolds inside it pushes the stages
// off the column exactly when the reviewer is deciding which one to send, so the
// rail keeps one compact line and this holds the rest — anchored under that line
// on a desktop, standing on the bottom edge of a phone, the way every other
// picker in the app resolves the same choice.
//
// Repaints are the house's: the panel is rendered to markup, and an unchanged
// render moves nothing at all. When it does change, only the disagreements are
// written, so the select the reviewer just used is still the select they are in.

import { patchElement } from "./domPatch.js";
import { assignmentPanelHtml } from "./issueRender.js";

/** The width below which a panel anchored to anything is a panel squeezed
 *  against a frame edge — the same width the panes stack at. */
export const NARROW_VIEWPORT = 720;

const PANEL_WIDTH = 340;
const PANEL_MARGIN = 8;
const ANCHOR_GAP = 6;
const ASSUMED_PANEL_HEIGHT = 340;

/** Every field the panel carries, and the key on the assignment it writes. */
const FIELDS = [
  ["#assignworktree", "worktree"],
  ["#assignworktreeid", "worktreeId"],
  ["#assignagent", "agent"],
  ["#assignbase", "base"],
  ["#assignprovider", "provider"],
  ["#assignmodel", "model"],
  ["#assigneffort", "effort"],
];

/**
 * Where the panel sits. Pure — a rect and a viewport in, coordinates out.
 *
 * Under the control when the frame has room under it, above it when it has not:
 * the control is the LAST row of the rail, so hanging downwards is the case
 * that does not fit, not the case that does. On a phone there is no room to
 * anchor into at all, and the panel stands on the bottom edge instead.
 */
export function panelWidth(viewport) {
  return Math.min(PANEL_WIDTH, viewport.width - PANEL_MARGIN * 2);
}

export function panelPlacement(rect, viewport, panelHeight = ASSUMED_PANEL_HEIGHT) {
  if (viewport.width <= NARROW_VIEWPORT) return { atBottom: true };
  const width = panelWidth(viewport);
  const left = Math.max(PANEL_MARGIN, Math.min(rect.left, viewport.width - width - PANEL_MARGIN));
  const below = rect.bottom + ANCHOR_GAP;
  const fitsBelow = below + panelHeight <= viewport.height - PANEL_MARGIN;
  const top = fitsBelow ? below : Math.max(PANEL_MARGIN, rect.top - ANCHOR_GAP - panelHeight);
  return { atBottom: false, left, top, width };
}

const EMPTY_RECT = { top: 0, bottom: 0, left: 0 };

/**
 * openAssignmentOverlay(options) → { update, close, isOpen, busy, element }.
 *
 * The caller owns the assignment: `getAssignment()` reads it, `setAssignment()`
 * is handed the next one, and nothing here mutates what it was given.
 * `getAnchor()` is re-read on every placement because the rail repaints on a
 * poll and hands back a different button each time. `onClose()` fires exactly
 * once, however the overlay was dismissed.
 */
export function openAssignmentOverlay({
  getAnchor = () => null,
  getAssignment,
  setAssignment = () => {},
  getCatalog = () => ({}),
  getWorktrees = () => [],
  onClose = () => {},
}) {
  const scrim = document.createElement("div");
  scrim.className = "assign-scrim";
  const panel = document.createElement("div");
  panel.className = "assign-pop";
  panel.tabIndex = -1;
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Assignment");
  const inner = document.createElement("div");
  inner.className = "assign-pop-in";
  panel.appendChild(inner);
  scrim.appendChild(panel);
  document.body.appendChild(scrim);

  let closed = false;
  let painted = null;

  const place = () => {
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const narrow = viewport.width <= NARROW_VIEWPORT;
    scrim.classList.toggle("at-bottom", narrow);
    panel.classList.toggle("at-bottom", narrow);
    if (narrow) {
      panel.style.top = "";
      panel.style.left = "";
      panel.style.width = "";
      return;
    }
    // Wear the width first: what the panel is wide is what decides how tall it
    // is, and how tall it is decides which side of the control it hangs on.
    panel.style.width = `${panelWidth(viewport)}px`;
    const anchor = getAnchor();
    const rect = anchor && anchor.getBoundingClientRect ? anchor.getBoundingClientRect() : EMPTY_RECT;
    const at = panelPlacement(rect, viewport, panel.offsetHeight || ASSUMED_PANEL_HEIGHT);
    panel.style.top = `${at.top}px`;
    panel.style.left = `${at.left}px`;
  };

  /** Put every field where the held assignment says, without writing a value a
   *  field already carries — assigning an input's own value back to it is what
   *  drops the caret to the end of what someone is typing.
   *
   *  An assignment that names nothing for a field is not an empty field: it is
   *  the catalog's own default, which the markup has already selected. Only a
   *  value the reviewer chose is written back over it — and only one the field
   *  can actually show: a select handed a value none of its options carries
   *  goes blank, which is how a preference the offer no longer holds would
   *  empty the control the markup had already clamped. */
  const showable = (field, value) =>
    field.tagName !== "SELECT" || [...field.options].some((option) => option.value === value);

  const syncValues = () => {
    const assignment = getAssignment();
    for (const [selector, key] of FIELDS) {
      const field = inner.querySelector(selector);
      if (!field) continue;
      const wanted = assignment[key] == null ? "" : String(assignment[key]);
      if (wanted !== "" && field.value !== wanted && showable(field, wanted)) field.value = wanted;
    }
  };

  const wire = () => {
    for (const [selector, key] of FIELDS) {
      const field = inner.querySelector(selector);
      if (!field) continue;
      field.onchange = () => {
        setAssignment({ ...getAssignment(), [key]: field.value });
        paint();
      };
      if (field.tagName === "INPUT")
        field.oninput = () => {
          setAssignment({ ...getAssignment(), [key]: field.value });
        };
    }
    const done = inner.querySelector("[data-assign-close]");
    if (done) done.onclick = () => close();
  };

  const paint = () => {
    if (closed) return;
    const markup = assignmentPanelHtml({
      assignment: getAssignment(),
      catalog: getCatalog(),
      worktrees: getWorktrees(),
    });
    // An unchanged render is a no-op: nothing is parsed, nothing is patched, and
    // the field under the reviewer's cursor is never handed back to them as a
    // different node.
    if (markup === painted) return;
    const staging = document.createElement("div");
    staging.className = inner.className;
    staging.innerHTML = markup;
    patchElement(inner, staging);
    painted = markup;
    syncValues();
    wire();
  };

  const onKeydown = (event) => {
    if (event.key !== "Escape") return;
    // Capture-phase + stopPropagation so a sheet or drawer underneath never sees
    // the press that shut this.
    event.stopPropagation();
    close();
  };

  function close() {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKeydown, { capture: true });
    window.removeEventListener("resize", place);
    scrim.remove();
    onClose();
  }

  scrim.onclick = (event) => {
    if (event.target === scrim) close();
  };
  document.addEventListener("keydown", onKeydown, { capture: true });
  window.addEventListener("resize", place);

  paint();
  place();
  panel.focus();

  return {
    /** Repaint from whatever the caller now holds — the catalog and the branch
     *  list arrive after the overlay is already open. */
    update: paint,
    close,
    isOpen: () => !closed,
    /** The reviewer is in one of the fields, so the surface behind stands down.
     *  The panel itself does not count: it takes focus on open for the keyboard,
     *  and that is not someone mid-choice. */
    busy: () => {
      const focused = document.activeElement;
      return Boolean(focused && focused !== panel && panel.contains(focused));
    },
    element: panel,
  };
}
