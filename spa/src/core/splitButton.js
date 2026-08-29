// A reusable GitHub-style split button: a primary (default) action plus a caret
// that opens a menu of the remaining actions. Generalizes the git split button
// once inlined in views/task.js. Pure markup + thin wiring; every user-supplied
// string is escaped.

import { esc } from "./text.js";

/** The app's button vocabulary a split button can be painted in: the accent
 *  primary (the default — a surface's decisive verb) or the mini secondary the
 *  dense toolbars use. Nothing else: a split button is a button, and it reads
 *  like every other one. */
const VARIANT_BUTTON_CLASS = {
  primary: "btn primary",
  mini: "btn mini",
};

/** Pure markup for a GitHub-style split button. options[0] is the default.
 *  option: { id, label, menuLabel?, description, busyLabel, danger? }
 *  With one option: a plain button, no caret, no menu. All strings escaped.
 *
 *  `danger` marks the option's MENU item, never the button chrome: a
 *  destructive verb says what it costs in the confirmation it opens, and a
 *  primary button painted half-accent half-red reads as neither.
 *
 *  `primaryId` names the default button for a caller that wires it by id
 *  rather than through `mountSplitButton` — the composer's send, which is one
 *  button in two shapes and must be found by one lookup either way. */
export function splitButtonMarkup(options, { variant = "primary", primaryId = "" } = {}) {
  const buttonClass = VARIANT_BUTTON_CLASS[variant];
  if (!buttonClass) throw new Error(`unknown split button variant: ${variant}`);
  const primary = options[0];
  const primaryButton = `<button class="${buttonClass}"${primaryId ? ` id="${esc(primaryId)}"` : ""} data-action="${esc(primary.id)}">${esc(primary.label ?? primary.menuLabel)}</button>`;
  if (options.length === 1) return `<div class="splitbtn">${primaryButton}</div>`;
  const items = options
    .map(
      (o) =>
        `<div class="mi${o.danger ? " danger" : ""}" data-action="${esc(o.id)}"><span class="mt">${esc(o.menuLabel ?? o.label)}</span><span class="md">${esc(o.description)}</span></div>`,
    )
    .join("");
  return `<div class="splitbtn">${primaryButton}<button class="${buttonClass} caret" title="More actions">▾</button><div class="splitmenu" hidden>${items}</div></div>`;
}

/** Pure single-flight latch: begin() arms and returns true, or returns false
 *  when a flight is already in progress; end() re-arms; active() peeks at the
 *  latch (callers freeze repaints while a flight is running). */
export function createSingleFlight() {
  let inFlight = false;
  return {
    begin() {
      if (inFlight) return false;
      inFlight = true;
      return true;
    },
    end() {
      inFlight = false;
    },
    active() {
      return inFlight;
    },
  };
}

/** Wire the caret and the menu of a split button already in the DOM: the caret
 *  toggles it, a press outside closes it, and choosing an item closes it and
 *  reports the option's id. Returns `{ closeMenu }` for a caller that has to
 *  shut it for its own reasons — a press that starts working, say. A container
 *  holding a lone button (no caret, no menu) wires nothing and the close is a
 *  no-op.
 *
 *  Split out of `mountSplitButton` because the composer's send is a split
 *  button whose press is NOT a single-flight action with a busy label: it is a
 *  submit that restores its own button, and re-rendering it under the poll is
 *  the composer's business. What both share is the menu. */
export function mountSplitMenu(container, { onChoose }) {
  const caret = container.querySelector(".caret");
  const menu = container.querySelector(".splitmenu");

  // The open menu's outside-press watch. It is armed in the same event cycle as
  // the click that opens the menu — deferring it to a macrotask loses the race
  // against a real pointer, whose press can land before the timer runs, so the
  // menu shuts the instant it appears. Arming it immediately is safe because a
  // press anywhere inside the split button (the caret that toggles it, the item
  // being reached for) is not outside.
  let stopWatchingOutsidePress = null;
  const closeMenu = () => {
    if (menu) menu.hidden = true;
    if (stopWatchingOutsidePress) stopWatchingOutsidePress();
  };
  const openMenu = () => {
    menu.hidden = false;
    if (stopWatchingOutsidePress) return;
    const onOutsidePress = (event) => {
      if (container.querySelector(".splitbtn")?.contains(event.target)) return;
      closeMenu();
    };
    document.addEventListener("pointerdown", onOutsidePress);
    stopWatchingOutsidePress = () => {
      document.removeEventListener("pointerdown", onOutsidePress);
      stopWatchingOutsidePress = null;
    };
  };

  if (caret && menu) {
    caret.onclick = (event) => {
      event.stopPropagation();
      if (caret.disabled) return;
      if (menu.hidden) openMenu();
      else closeMenu();
    };
    menu.querySelectorAll(".mi").forEach(
      (mi) =>
        (mi.onclick = () => {
          closeMenu();
          onChoose(mi.dataset.action);
        }),
    );
  }
  return { closeMenu };
}

/** What each container was last mounted from. A poll-driven caller remounts the
 *  same button over and over, and the markup is what says whether that remount
 *  would change anything at all. Keyed weakly: a container that goes away takes
 *  its entry with it. */
const mountedMarkup = new WeakMap();

/** Render into `container` and wire behavior. `run(optionId)` is awaited; while
 *  in flight the primary button and caret are disabled, the menu stays closed,
 *  and further invokes are ignored (single flight — no concurrent destructive
 *  RPCs); rejection restores label + enabled (the caller owns error display);
 *  resolution leaves both disabled (the caller repaints/navigates). The caret
 *  toggles the menu; a pointerdown outside closes it; a menu item runs its
 *  option through the same primary button. Callers whose surface remounts the
 *  button while an action can be pending (poll-driven repaints) pass a shared
 *  `flight` latch so a remount can never re-arm a fresh one mid-flight.
 *  `variant` picks the button vocabulary — "primary" (default) or "mini". */
export function mountSplitButton(container, { options, run, variant = "primary", flight = createSingleFlight() }) {
  const markup = splitButtonMarkup(options, { variant });
  // A repaint that would change nothing must not close the menu the user just
  // opened, nor swap a busy button for a fresh one: a click in progress
  // outranks a poll tick, which lands again once the menu is shut. Options that
  // actually moved still rebuild — what the button offers has changed.
  const held = container.querySelector(".splitmenu:not([hidden])") || (flight.active() && container.querySelector(".splitbtn"));
  if (held && mountedMarkup.get(container) === markup) return;
  container.innerHTML = markup;
  mountedMarkup.set(container, markup);
  const byId = Object.fromEntries(options.map((o) => [o.id, o]));
  const primary = container.querySelector(".btn:not(.caret)");
  const caret = container.querySelector(".caret");
  const { closeMenu } = mountSplitMenu(container, { onChoose: (optionId) => invoke(optionId) });

  const invoke = async (optionId) => {
    if (!flight.begin()) return;
    const option = byId[optionId];
    closeMenu();
    primary.disabled = true;
    if (caret) caret.disabled = true;
    const restoreLabel = primary.textContent;
    primary.textContent = option.busyLabel || "working…";
    try {
      await run(optionId);
      // Resolution: leave primary + caret disabled — the caller repaints or navigates.
    } catch {
      // Rejection: the caller has shown the error; restore the buttons so the
      // user can retry a different action.
      primary.disabled = false;
      if (caret) caret.disabled = false;
      primary.textContent = restoreLabel;
    } finally {
      flight.end();
    }
  };

  primary.onclick = () => invoke(primary.dataset.action);
}
