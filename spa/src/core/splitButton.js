// A reusable GitHub-style split button: a primary (default) action plus a caret
// that opens a menu of the remaining actions. Generalizes the git split button
// once inlined in views/task.js. Pure markup + thin wiring; every user-supplied
// string is escaped.

import { hide, reveal } from "./motion.js";
import { esc } from "./text.js";

const MENU_MOVE = { axis: "height" };

export const SPLIT_BUTTON_SELECTOR = ".splitbtn";
const CARET_SELECTOR = ".caret";
const SPLIT_MENU_SELECTOR = ".splitmenu";

/** The app's button vocabulary a split button can be painted in: the accent
 *  primary (the default — a surface's decisive verb) or the mini secondary the
 *  dense toolbars use. Nothing else: a split button is a button, and it reads
 *  like every other one. */
const VARIANT_BUTTON_CLASS = {
  primary: "btn primary",
  mini: "btn mini",
};

/** The menu half's rows: one per option, its name and what it does.
 *
 *  `danger` marks a destructive verb where it is READ, in the list beside the
 *  ones it is chosen over. `selected` marks the one in use — a menu that is a
 *  selection rather than a verb (the composer's model menu) has to say which
 *  answer is standing. */
function menuItemsHtml(options) {
  return options
    .map(
      (o) =>
        `<div class="mi${o.danger ? " danger" : ""}${o.selected ? " on" : ""}" data-action="${esc(o.id)}"><span class="mt">${esc(o.menuLabel ?? o.label)}</span><span class="md">${esc(o.description)}</span></div>`,
    )
    .join("");
}

/** Pure markup for the menu half ALONE: one button that opens it, and the same
 *  rows a split button's caret drops. For a menu that is a selection rather
 *  than a verb — there is no default action to press, so there is no primary
 *  button to press it with. Wire it with `mountSplitMenu`. */
export function menuButtonMarkup(label, options, { title = "", icon = false, arrow = true } = {}) {
  const titled = title ? ` title="${esc(title)}" aria-label="${esc(title)}"` : "";
  const opener = icon
    ? `<button type="button" class="iconbtn caret"${titled}>${esc(label)}</button>`
    : `<button type="button" class="btn mini caret"${titled}>${esc(label)}${arrow ? ' <span class="disclosure-caret" aria-hidden="true">▾</span>' : ""}</button>`;
  return `<div class="splitbtn${icon ? " splitbtn-icon" : ""}">
    ${opener}
    <div class="splitmenu" hidden>${menuItemsHtml(options)}</div>
  </div>`;
}

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
  return `<div class="splitbtn">${primaryButton}<button class="${buttonClass} caret" title="More actions" aria-label="More actions"><span class="disclosure-caret" aria-hidden="true">▾</span></button><div class="splitmenu" hidden>${menuItemsHtml(options)}</div></div>`;
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

const MENU_GAP_PX = 6;

function scrollingAncestorOf(element) {
  for (let ancestor = element.parentElement; ancestor && ancestor !== document.body; ancestor = ancestor.parentElement) {
    const overflowY = getComputedStyle(ancestor).overflowY;
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "hidden") return ancestor;
  }
  return null;
}

function placeMenuFromButtonBox(menu, buttonBox, { width: menuWidth, height: menuHeight }) {
  const opensAbove = buttonBox.top - MENU_GAP_PX >= menuHeight;
  const viewportGap = 8;
  const left = Math.max(viewportGap, Math.min(buttonBox.right - menuWidth, window.innerWidth - menuWidth - viewportGap));
  menu.style.position = "fixed";
  menu.style.left = `${left}px`;
  menu.style.right = "auto";
  menu.style.top = opensAbove ? "auto" : `${buttonBox.bottom + MENU_GAP_PX}px`;
  menu.style.bottom = opensAbove ? `${window.innerHeight - buttonBox.top + MENU_GAP_PX}px` : "auto";
  return opensAbove
    ? { left, edge: "bottom", bottom: buttonBox.top - MENU_GAP_PX }
    : { left, edge: "top", top: buttonBox.bottom + MENU_GAP_PX };
}

function menuSizeWhenShown(menu) {
  const wasHidden = menu.hidden;
  menu.hidden = false;
  const rendered = menu.getBoundingClientRect();
  const size = {
    width: rendered.width || menu.offsetWidth,
    height: rendered.height || menu.offsetHeight,
  };
  menu.hidden = wasHidden;
  return size;
}

/** `position:fixed` is viewport-relative until an ancestor has transform,
 * filter, or backdrop-filter. Glass headers use the latter, making the same
 * coordinates relative to the header and sending a nominally open menu past
 * the viewport. Measure where the browser actually put it and compensate. */
function correctFixedMenuOffset(menu, wanted) {
  const wasHidden = menu.hidden;
  menu.hidden = false;
  const placed = menu.getBoundingClientRect();
  if (!placed.width || !placed.height) {
    menu.hidden = wasHidden;
    return;
  }
  const scaleX = placed.width / menu.offsetWidth || 1;
  const scaleY = placed.height / menu.offsetHeight || 1;
  menu.hidden = wasHidden;
  menu.style.left = `${Number.parseFloat(menu.style.left) + (wanted.left - placed.left) / scaleX}px`;
  if (wanted.edge === "top") {
    menu.style.top = `${Number.parseFloat(menu.style.top) + (wanted.top - placed.top) / scaleY}px`;
  } else {
    menu.style.bottom = `${Number.parseFloat(menu.style.bottom) + (placed.bottom - wanted.bottom) / scaleY}px`;
  }
}

function liftMenuOutOfScroll(container, menu, closeMenu) {
  const buttonBox = container.querySelector(SPLIT_BUTTON_SELECTOR).getBoundingClientRect();
  const wanted = placeMenuFromButtonBox(menu, buttonBox, menuSizeWhenShown(menu));
  correctFixedMenuOffset(menu, wanted);
  const onViewportMoved = () => closeMenu();
  document.addEventListener("scroll", onViewportMoved, { capture: true });
  window.addEventListener("resize", onViewportMoved);
  return () => {
    document.removeEventListener("scroll", onViewportMoved, { capture: true });
    window.removeEventListener("resize", onViewportMoved);
    menu.style.position = "";
    menu.style.left = "";
    menu.style.top = "";
    menu.style.bottom = "";
    menu.style.right = "";
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
  const caret = container.querySelector(CARET_SELECTOR);
  const menu = container.querySelector(SPLIT_MENU_SELECTOR);

  // The open menu's outside-press watch. It is armed in the same event cycle as
  // the click that opens the menu — deferring it to a macrotask loses the race
  // against a real pointer, whose press can land before the timer runs, so the
  // menu shuts the instant it appears. Arming it immediately is safe because a
  // press anywhere inside the split button (the caret that toggles it, the item
  // being reached for) is not outside.
  let stopWatchingOutsidePress = null;
  let settleLiftedMenu = null;
  let menuIsOpen = false;
  const closeMenu = () => {
    if (!menu) return Promise.resolve();
    menuIsOpen = false;
    if (stopWatchingOutsidePress) stopWatchingOutsidePress();
    return hide(menu, MENU_MOVE).then(() => {
      if (menuIsOpen || !settleLiftedMenu) return;
      settleLiftedMenu();
      settleLiftedMenu = null;
    });
  };
  const openMenu = () => {
    menuIsOpen = true;
    if (!settleLiftedMenu && scrollingAncestorOf(menu)) settleLiftedMenu = liftMenuOutOfScroll(container, menu, closeMenu);
    reveal(menu, MENU_MOVE);
    if (stopWatchingOutsidePress) return;
    const onOutsidePress = (event) => {
      if (container.querySelector(SPLIT_BUTTON_SELECTOR)?.contains(event.target)) return;
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
      if (menuIsOpen) closeMenu();
      else openMenu();
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

const menuMountedInContainer = new WeakMap();

export function mountMenuIfChanged(container, markup, { onChoose }) {
  const mounted = menuMountedInContainer.get(container);
  if (mounted && mounted.markup === markup) return mounted.closeMenu;
  if (mounted) mounted.closeMenu();
  container.innerHTML = markup;
  const { closeMenu } = mountSplitMenu(container, { onChoose });
  menuMountedInContainer.set(container, { markup, closeMenu });
  return closeMenu;
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
  const held = container.querySelector(`${SPLIT_MENU_SELECTOR}:not([hidden])`) || (flight.active() && container.querySelector(SPLIT_BUTTON_SELECTOR));
  if (held && mountedMarkup.get(container) === markup) return;
  container.innerHTML = markup;
  mountedMarkup.set(container, markup);
  const byId = Object.fromEntries(options.map((o) => [o.id, o]));
  const primary = container.querySelector(".btn:not(.caret)");
  const caret = container.querySelector(CARET_SELECTOR);
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
