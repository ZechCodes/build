// A reusable GitHub-style split button: a primary (default) action plus a caret
// that opens a menu of the remaining actions. Generalizes the git split button
// once inlined in views/task.js. Pure markup + thin wiring; every user-supplied
// string is escaped.

import { hide, motionSettled, reveal } from "./motion.js";
import { esc } from "./text.js";
import { adjustsMenuSlider, commitMenuSlider, commitMenuSliders, menuSliderMarkup, MENU_SLIDER_SELECTOR, mountMenuSliders, resetMenuSliders } from "./menuSlider.js";
import { patchSplitMenu } from "./splitMenuPatch.js";

const MENU_MOVE = { axis: "height" };

export const SPLIT_BUTTON_SELECTOR = ".splitbtn";
const CARET_SELECTOR = ".caret";
const SPLIT_MENU_SELECTOR = ".splitmenu";
const MENU_ITEM_SELECTOR = ".mi";
const MENU_FOCUS_SELECTOR = `${MENU_ITEM_SELECTOR}, ${MENU_SLIDER_SELECTOR}`;
const MENU_NOTE_SELECTOR = ".menu-note";

/** The app's button vocabulary a split button can be painted in: the accent
 *  primary (the default — a surface's decisive verb) or the mini secondary the
 *  dense toolbars use. Nothing else: a split button is a button, and it reads
 *  like every other one. */
const VARIANT_BUTTON_CLASS = {
  primary: "btn primary",
  mini: "btn mini",
};

/** A row's word to assistive tech. One that carries a standing answer
 *  (`selected` is a boolean) is one of a radio set: the menu is a selection,
 *  and a screen reader says which answer stands. Any other row is a plain
 *  item. */
const isAnswer = (option) => typeof option.selected === "boolean";
const roleAttributes = (option) =>
  isAnswer(option) ? ` role="menuitemradio" aria-checked="${option.selected}"` : ' role="menuitem"';

/** The menu half's rows: one per option, its name and what it does. Each is
 *  reachable from the keyboard once the menu is open — `mountSplitMenu` walks
 *  them with the arrows — and out of the tab order otherwise.
 *
 *  `danger` marks a destructive verb where it is READ, in the list beside the
 *  ones it is chosen over. `selected` marks the one in use — a menu that is a
 *  selection rather than a verb (the composer's model menu) has to say which
 *  answer is standing. */
function menuItemsHtml(options) {
  return options
    .map(
      (o) =>
        `<div class="mi${o.danger ? " danger" : ""}${o.selected ? " on" : ""}" data-action="${esc(o.id)}"${roleAttributes(o)} tabindex="-1"><span class="mt">${esc(o.menuLabel ?? o.label)}</span><span class="md">${esc(o.description)}</span></div>`,
    )
    .join("");
}

/** A menu in sections. Each group `{ id, label, options }` is headed by what
 *  it holds, and named for assistive tech by the group itself, so the heading
 *  is read once rather than twice. The conversation head's ⋮
 *  (core/agentRail.js) is one: what the agent opened, then the conversation's
 *  settings, each of those a radio set. */
function menuGroupsHtml(groups) {
  return groups
    .map(
      (group) =>
        `<div class="menu-group" role="group" aria-label="${esc(group.label)}" data-group="${esc(group.id)}"><div class="menu-group-title" aria-hidden="true">${esc(group.label)}</div>${menuGroupControl(group)}</div>`,
    )
    .join("");
}

const menuGroupControl = (group) => group.control === "slider" ? menuSliderMarkup(group) : menuItemsHtml(group.options);

/** A line of text at the menu's foot that is not a choice: no action, out of
 *  the keyboard's walk (which moves between `.mi` rows only), and read to
 *  assistive tech as the menu's description rather than as an item. Each
 *  gets its own id, since the menu names it by one. */
let menuNoteCount = 0;
const menuNoteHtml = (note, id) =>
  note ? `<div class="menu-note model-update-note" id="${id}" role="none">${esc(note)}</div>` : "";

const menuHtml = (rowsHtml, name, note = "") => {
  const noteId = note ? `menu-note-${(menuNoteCount += 1)}` : "";
  const described = noteId ? ` aria-describedby="${noteId}"` : "";
  return `<div class="splitmenu" hidden role="menu"${name ? ` aria-label="${esc(name)}"` : ""}${described}>${rowsHtml}${menuNoteHtml(note, noteId)}</div>`;
};

/** The opener's word to assistive tech: it holds a menu, shut until pressed.
 *  `mountSplitMenu` keeps the second half true. */
const POPUP_ATTRIBUTES = ' aria-haspopup="menu" aria-expanded="false"';

function menuButtonHtml(label, rowsHtml, { title = "", icon = false, arrow = true, note = "" } = {}) {
  const titled = title ? ` title="${esc(title)}" aria-label="${esc(title)}"` : "";
  const opener = icon
    ? `<button type="button" class="iconbtn caret"${titled}${POPUP_ATTRIBUTES}>${esc(label)}</button>`
    : `<button type="button" class="btn mini caret"${titled}${POPUP_ATTRIBUTES}>${esc(label)}${arrow ? ' <span class="disclosure-caret" aria-hidden="true">▾</span>' : ""}</button>`;
  return `<div class="splitbtn${icon ? " splitbtn-icon" : ""}">
    ${opener}
    ${menuHtml(rowsHtml, title, note)}
  </div>`;
}

/** Pure markup for the menu half ALONE: one button that opens it, and the same
 *  rows a split button's caret drops. For a menu that is a selection rather
 *  than a verb — there is no default action to press, so there is no primary
 *  button to press it with. Wire it with `mountSplitMenu`. `shape.note` puts
 *  a line of text under the rows (`menuNoteHtml`). */
export function menuButtonMarkup(label, options, shape = {}) {
  return menuButtonHtml(label, menuItemsHtml(options), shape);
}

/** `menuButtonMarkup` for a menu in sections: `groups` in place of options,
 *  each `{ id, label, options }` (`menuGroupsHtml`). */
export function groupedMenuButtonMarkup(label, groups, shape = {}) {
  return menuButtonHtml(label, menuGroupsHtml(groups), shape);
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
  return `<div class="splitbtn">${primaryButton}<button class="${buttonClass} caret" title="More actions" aria-label="More actions"${POPUP_ATTRIBUTES}><span class="disclosure-caret" aria-hidden="true">▾</span></button>${menuHtml(menuItemsHtml(options), "More actions")}</div>`;
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
/** The margin a lifted menu keeps between itself and every edge of the
 *  viewport, so no corner of it is ever cut off by one. */
const VIEWPORT_GAP_PX = 8;

function scrollingAncestorOf(element) {
  for (let ancestor = element.parentElement; ancestor && ancestor !== document.body; ancestor = ancestor.parentElement) {
    const overflowY = getComputedStyle(ancestor).overflowY;
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "hidden") return ancestor;
  }
  return null;
}

/** The bottom edge a lifted menu is kept above: the viewport's, cut to the
 *  region its caller keeps it within where that region's box is known. The
 *  conversation panel stops on the bubble strip at phone width and the strip
 *  is drawn over it, so a menu run to the viewport's foot had its last rows
 *  under the strip (#124); the rail names its panel. A region that reports no
 *  box (one not laid out) says nothing, and the viewport stands. */
function bottomBoundOf(region) {
  const edge = region ? region.getBoundingClientRect().bottom : 0;
  return edge > 0 ? Math.min(window.innerHeight, edge) : window.innerHeight;
}

/** Which way a menu falls from its button: DOWN, the direction the stylesheet
 *  writes every menu in, unless the menu does not fit below the button (above
 *  `bound`) and does fit above it. Preferring above whenever there was room —
 *  the rule this replaced — opened the conversation head's menu upward off
 *  the top of the screen, because the room it measured was the whole page
 *  above a header that sits at the top of it. */
function menuOpensAbove(buttonBox, menuHeight, bound) {
  const roomBelow = bound - buttonBox.bottom - MENU_GAP_PX - VIEWPORT_GAP_PX;
  const roomAbove = buttonBox.top - MENU_GAP_PX - VIEWPORT_GAP_PX;
  return menuHeight > roomBelow && roomAbove >= menuHeight;
}

/** An edge offset held inside the viewport's gutter, so a menu taller or wider
 *  than the room its button left it is MOVED to fit rather than hung off the
 *  edge. One taller than the viewport itself starts at the near gutter — there
 *  is nowhere left to put the rest of it. */
function clampedToViewport(offset, extent, bound) {
  return Math.max(VIEWPORT_GAP_PX, Math.min(offset, bound - extent - VIEWPORT_GAP_PX));
}

function placeMenuFromButtonBox(menu, buttonBox, { width: menuWidth, height: menuHeight }, bound) {
  const left = clampedToViewport(buttonBox.right - menuWidth, menuWidth, window.innerWidth);
  menu.style.position = "fixed";
  menu.style.left = `${left}px`;
  menu.style.right = "auto";
  if (menuOpensAbove(buttonBox, menuHeight, bound)) {
    // Anchored by its bottom edge: a menu opening upward has to GROW upward as
    // the reveal animates its height, away from the button rather than over it.
    // The inset is from the viewport's foot, so the bound's edge is at least
    // its own gutter above that.
    const bottom = Math.max(
      clampedToViewport(window.innerHeight - buttonBox.top + MENU_GAP_PX, menuHeight, window.innerHeight),
      window.innerHeight - bound + VIEWPORT_GAP_PX,
    );
    menu.style.top = "auto";
    menu.style.bottom = `${bottom}px`;
    return { left, edge: "bottom", bottom: window.innerHeight - bottom };
  }
  const top = clampedToViewport(buttonBox.bottom + MENU_GAP_PX, menuHeight, bound);
  menu.style.top = `${top}px`;
  menu.style.bottom = "auto";
  return { left, edge: "top", top };
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

function liftMenuOutOfScroll(container, menu, closeMenu, region) {
  const buttonBox = container.querySelector(SPLIT_BUTTON_SELECTOR).getBoundingClientRect();
  const bound = bottomBoundOf(region);
  // Taller than the room there is, the menu scrolls inside it (`.splitmenu`
  // scrolls on its y axis) rather than running off the edge.
  menu.style.maxHeight = `${bound - 2 * VIEWPORT_GAP_PX}px`;
  const wanted = placeMenuFromButtonBox(menu, buttonBox, menuSizeWhenShown(menu), bound);
  correctFixedMenuOffset(menu, wanted);
  // A scroll inside the menu itself is the reader reading it, not the page
  // moving out from under the menu.
  const onViewportMoved = (event) => {
    if (event.type === "scroll" && menu.contains(event.target)) return;
    closeMenu();
  };
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
    menu.style.maxHeight = "";
  };
}

/** One watch per container for focus leaving it. A caller that re-renders
 *  into the same slot mounts again and again (the composer's model menu), and
 *  a listener added each time would pile up, every old one shutting a menu
 *  that is no longer there. `onfocusout` would do the same in one line, but
 *  jsdom does not carry it, and the DOM suites are how the wiring is proven. */
const focusWatchers = new WeakMap();
const menuClosers = new WeakMap();

/** Close every menu owned by a pane before its host is hidden. */
export function closeSplitMenusWithin(root) {
  for (const button of root.querySelectorAll(SPLIT_BUTTON_SELECTOR)) {
    menuClosers.get(button.parentElement)?.();
  }
}

function watchFocusLeaving(container, onLeft) {
  const previous = focusWatchers.get(container);
  if (previous) container.removeEventListener("focusout", previous);
  container.addEventListener("focusout", onLeft);
  focusWatchers.set(container, onLeft);
}

/** A row's top, measured from the menu's scrolling origin: its offsets summed
 *  up to the menu, whichever of its ancestors are positioned. */
function rowTopWithin(menu, row) {
  let top = 0;
  for (let each = row; each && each !== menu && menu.contains(each); each = each.offsetParent) top += each.offsetTop;
  return top;
}

/** Where a row's sight starts: at the row, or at its group's heading when
 *  it is the group's first row (`menuGroupsHtml`) — a value without the name
 *  of its setting above it is a bare word. */
function sightTopOf(menu, row) {
  const opensGroup = row.parentElement !== menu && !row.previousElementSibling?.matches(MENU_ITEM_SELECTOR);
  return rowTopWithin(menu, opensGroup ? row.parentElement : row);
}

/** Scroll the menu, and only the menu, until the row is inside its visible
 *  area. A row takes focus with `preventScroll` so the page under a lifted
 *  menu stays where it is — but a menu taller than its bound scrolls inside
 *  itself (`liftMenuOutOfScroll`), and a row past its edge would otherwise
 *  take focus out of sight, where Enter chooses what the reader cannot see.
 *  Measured in layout units: the reveal animates the menu's height, so a
 *  scroll made while it plays can land wrong, and `focusRow` measures again
 *  once motion has settled. The menu's note is pinned over its foot
 *  (`menuNoteHtml`), so what is in sight stops at the note. */
function scrollRowIntoMenu(menu, row) {
  // The thumb takes focus, but the setting's selected word below it must be
  // in sight too. Its wrapper also brings the group heading.
  const box = row.matches(MENU_SLIDER_SELECTOR) ? row.closest(".menu-slider") : row;
  const above = sightTopOf(menu, box) - menu.scrollTop;
  const sightHeight = menu.clientHeight - (menu.querySelector(MENU_NOTE_SELECTOR)?.offsetHeight || 0);
  const below = rowTopWithin(menu, box) + box.offsetHeight - menu.scrollTop - sightHeight;
  if (above < 0) menu.scrollTop += above;
  else if (below > 0) menu.scrollTop += below;
}

/** The keys a menu answers. On its opener, the arrows open it with a row
 *  focused — ArrowDown the first, ArrowUp the last — and Escape shuts it. In
 *  the menu, the arrows walk the rows and wrap, Home and End jump, Enter and
 *  Space choose the focused row, and Escape shuts without choosing. Focus goes
 *  back to the opener whenever the menu shuts from the keyboard. Tab is left
 *  alone: focus leaving the menu shuts it (`mountSplitMenu`'s focusout).
 *
 *  Every row the keys land on is scrolled into sight (`scrollRowIntoMenu`):
 *  at once, and again once the menu's motion has settled, for a key pressed
 *  while the reveal is still playing. */
function menuKeyboard({ caret, menu, isOpen, openMenu, closeMenu, choose }) {
  const rows = () => [...menu.querySelectorAll(MENU_FOCUS_SELECTOR)];
  const focusedRow = () => rows().find((each) => each === document.activeElement);
  const showFocusedRow = () => {
    const row = focusedRow();
    if (row) scrollRowIntoMenu(menu, row);
  };
  const focusRow = (index) => {
    const all = rows();
    if (!all.length) return;
    all[((index % all.length) + all.length) % all.length].focus({ preventScroll: true });
    showFocusedRow();
    motionSettled().then(showFocusedRow);
  };
  // Shown at once, so the row can take focus; the reveal still plays over
  // the top of that, from nothing to its height.
  const openOnRow = (index) => {
    openMenu();
    menu.hidden = false;
    focusRow(index);
  };
  const shutToCaret = () => {
    closeMenu();
    caret.focus({ preventScroll: true });
  };
  const chooseFocused = () => {
    const row = focusedRow();
    if (row) choose(row);
  };
  const focusedIndex = () => rows().indexOf(document.activeElement);
  const onCaret = {
    ArrowDown: () => openOnRow(0),
    ArrowUp: () => openOnRow(-1),
    Escape: () => (isOpen() ? closeMenu() : false),
  };
  const onMenu = {
    ArrowDown: () => focusRow(focusedIndex() + 1),
    ArrowUp: () => focusRow(focusedIndex() - 1),
    Home: () => focusRow(0),
    End: () => focusRow(-1),
    Escape: shutToCaret,
    Enter: chooseFocused,
    " ": chooseFocused,
  };
  // A key the menu does not answer — or Escape with nothing open — is left to
  // whatever else is listening.
  const handling = (keys) => (event) => {
    if (adjustsMenuSlider(event)) return;
    if (!Object.hasOwn(keys, event.key) || keys[event.key]() === false) return;
    event.preventDefault();
    event.stopPropagation();
  };
  return { onCaretKey: handling(onCaret), onMenuKey: handling(onMenu) };
}

/** Wire the caret and the menu of a split button already in the DOM: the caret
 *  toggles it, a press outside closes it, focus leaving it closes it, the
 *  keyboard drives it (`menuKeyboard`), and choosing an item closes it and
 *  reports the option's id. Returns `{ closeMenu }` for a caller that has to
 *  shut it for its own reasons — a press that starts working, say. A container
 *  holding a lone button (no caret, no menu) wires nothing and the close is a
 *  no-op.
 *
 *  `keepWithin` names the region a lifted menu stays inside, by its bottom
 *  edge (`bottomBoundOf`): the rail's panel, which on a phone stops on the
 *  bubble strip that would otherwise cover the menu's last rows.
 *
 *  Split out of `mountSplitButton` because the composer's send is a split
 *  button whose press is NOT a single-flight action with a busy label: it is a
 *  submit that restores its own button, and re-rendering it under the poll is
 *  the composer's business. What both share is the menu. */
export function mountSplitMenu(container, { onChoose, onOpenChange = null, keepWithin = null }) {
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
  const sayExpanded = () => caret?.setAttribute("aria-expanded", String(menuIsOpen));
  const closeMenu = (announce = true) => {
    if (!menu) return Promise.resolve();
    resetMenuSliders(menu);
    const wasOpen = menuIsOpen;
    menuIsOpen = false;
    sayExpanded();
    if (wasOpen && announce) onOpenChange?.(false);
    if (stopWatchingOutsidePress) stopWatchingOutsidePress();
    return hide(menu, MENU_MOVE).then(() => {
      if (menuIsOpen || !settleLiftedMenu) return;
      settleLiftedMenu();
      settleLiftedMenu = null;
    });
  };
  menuClosers.set(container, closeMenu);
  // Lifted out of a scrolling ancestor once per opening, and put back once
  // the menu has shut (`closeMenu`).
  const liftIfScrolling = () => {
    if (settleLiftedMenu || !scrollingAncestorOf(menu)) return;
    settleLiftedMenu = liftMenuOutOfScroll(container, menu, closeMenu, keepWithin ? keepWithin() : null);
  };
  const openMenu = (announce = true) => {
    if (!menu || menuIsOpen) return;
    menuIsOpen = true;
    sayExpanded();
    if (announce) onOpenChange?.(true);
    liftIfScrolling();
    reveal(menu, MENU_MOVE);
    if (stopWatchingOutsidePress) return;
    const onOutsidePress = (event) => {
      if (container.querySelector(SPLIT_BUTTON_SELECTOR)?.contains(event.target)) return;
      commitMenuSliders(menu);
      closeMenu();
    };
    document.addEventListener("pointerdown", onOutsidePress);
    stopWatchingOutsidePress = () => {
      document.removeEventListener("pointerdown", onOutsidePress);
      stopWatchingOutsidePress = null;
    };
  };

  // Slider saves keep the menu and thumb focused through the cache repaint.
  // Other choices shut the menu and return focus to its opener.
  const choose = (row) => {
    if (row.matches(MENU_SLIDER_SELECTOR)) {
      row.focus({ preventScroll: true });
      void commitMenuSlider(row);
      return;
    }
    const optionId = row.dataset.action;
    closeMenu();
    caret.focus({ preventScroll: true });
    onChoose(optionId);
  };

  if (caret && menu) {
    const keys = menuKeyboard({ caret, menu, isOpen: () => menuIsOpen, openMenu, closeMenu, choose });
    caret.onclick = (event) => {
      event.stopPropagation();
      if (caret.disabled) return;
      if (menuIsOpen) closeMenu();
      else openMenu();
    };
    caret.onkeydown = keys.onCaretKey;
    menu.onkeydown = keys.onMenuKey;
    watchFocusLeaving(container, (event) => {
      if (menuIsOpen && !container.contains(event.relatedTarget)) closeMenu();
    });
    menu.querySelectorAll(MENU_ITEM_SELECTOR).forEach((mi) => (mi.onclick = () => choose(mi)));
    mountMenuSliders(menu, { choose, onCommit: onChoose });
  }
  return { closeMenu, openMenu, isOpen: () => menuIsOpen };
}

const menuMountedInContainer = new WeakMap();

export function mountMenuIfChanged(container, markup, { onChoose, keepWithin = null }) {
  const mounted = menuMountedInContainer.get(container);
  if (mounted && mounted.markup === markup) return mounted.closeMenu;
  if (mounted?.isOpen() && patchSplitMenu(container, mounted.markup, markup)) {
    mounted.markup = markup;
    return mounted.closeMenu;
  }
  if (mounted) mounted.closeMenu();
  // A remount under the reader's focus — the mark moved after a choice made
  // from the keyboard — hands focus to the new opener rather than dropping
  // it on the body.
  const hadFocus = container.contains(document.activeElement);
  container.innerHTML = markup;
  const { closeMenu, isOpen } = mountSplitMenu(container, { onChoose, keepWithin });
  menuMountedInContainer.set(container, { markup, closeMenu, isOpen });
  if (hadFocus) container.querySelector(CARET_SELECTOR)?.focus({ preventScroll: true });
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
