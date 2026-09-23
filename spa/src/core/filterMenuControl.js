// One filter menu, mounted: the press, the popover it opens, and the keyboard.
//
// Mounted ONCE and never re-created, which is the rule #43 put on this bar and
// the reason this control can exist at all: a popover the reader has open, a
// query they are half way through typing and the row they have walked to with
// the arrow keys are all state that lives in the DOM, and a bar that redrew
// itself would take every one of them. `update` says what is on offer and what
// is chosen; it never touches the press, the search box or the open state.
//
// The rows inside the popover ARE repainted — by key (core/patchList.js), so
// the row under the pointer survives a keystroke in the search box.
//
// The pattern is a combobox, not a menu: focus stays in the search box the
// whole time and the walked row is named by `aria-activedescendant`. That is
// what lets the arrow keys and the typing be the same gesture, and it is also
// what keeps a repaint from ever fighting for the focus.
//
// core/filterMenu.js holds every decision this makes; what is left here is the
// DOM and the listeners.

import { esc } from "./text.js";
import { patchList } from "./patchList.js";
import { watchUiState } from "./localUiState.js";
import {
  firstActive,
  menuPressLabel,
  menuRows,
  moveActive,
  searchLabel,
  toggleChoice,
} from "./filterMenu.js";

let mounted = 0;

const ICON_CHEVRON = `<svg class="fmenu-caret" viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;

const frameHtml = (id, name, multi) => `<div class="fmenu" data-filter-menu="${esc(name)}">
    <button class="fmenu-press" type="button" id="${id}-press"
      aria-haspopup="listbox" aria-expanded="false" aria-controls="${id}-pop">
      <span class="fmenu-press-label"></span>${ICON_CHEVRON}
    </button>
    <div class="fmenu-pop" id="${id}-pop" hidden role="group" aria-labelledby="${id}-press">
      <input class="fmenu-search" id="${id}-search" type="text" autocomplete="off" spellcheck="false"
        role="combobox" aria-expanded="true" aria-controls="${id}-rows" aria-autocomplete="list">
      <ul class="fmenu-rows" id="${id}-rows" role="listbox"${multi ? ' aria-multiselectable="true"' : ""}></ul>
      <p class="fmenu-none" hidden>Nothing matches that.</p>
      <div class="fmenu-foot">
        <button class="btn mini fmenu-clear" type="button">Clear</button>
      </div>
    </div>
  </div>`;

const rowHtml = (row, id, index) =>
  row.kind === "group"
    ? `<li class="fmenu-group" role="presentation">${esc(row.label)}</li>`
    : `<li class="fmenu-row" id="${id}-r${index}" role="option" aria-selected="${row.checked}" data-value="${esc(row.value)}">
        <span class="fmenu-tick" aria-hidden="true"></span><span class="fmenu-label">${esc(row.label)}</span>
      </li>`;

/**
 * Mount one filter menu into `host`.
 *
 * `name` is what it is called — on the press while nothing is chosen it is the
 * empty row's words instead, and in the search box's accessible name it is
 * this. `multi` decides whether a press adds to the selection or replaces it,
 * and `summary` how a selection of several is said (core/filterMenu.js).
 * `onChange` is handed the whole new selection, as a list, every time.
 */
export function mountFilterMenu(host, { name, label, multi = false, summary = "first", invent = null, onChange, cacheAddress = null }) {
  mounted += 1;
  const id = `fmenu-${mounted}`;
  host.insertAdjacentHTML("beforeend", frameHtml(id, name, multi));

  const root = host.lastElementChild;
  const press = root.querySelector(".fmenu-press");
  const pressLabel = root.querySelector(".fmenu-press-label");
  const pop = root.querySelector(".fmenu-pop");
  const search = root.querySelector(".fmenu-search");
  const list = root.querySelector(".fmenu-rows");
  const nothing = root.querySelector(".fmenu-none");
  const clear = root.querySelector(".fmenu-clear");

  search.setAttribute("aria-label", searchLabel(label));
  search.placeholder = searchLabel(label);
  press.setAttribute("aria-label", label);

  let options = [];
  let chosen = [];
  let rows = [];
  let active = -1;
  let open = false;
  let record = null;
  let focusSearch = false;
  let restorePress = false;

  // ---- painting ------------------------------------------------------------

  const paintPress = () => {
    pressLabel.textContent = menuPressLabel({ name: label, options, chosen, summary });
    press.classList.toggle("is-set", chosen.length > 0);
  };

  const paintActive = () => {
    [...list.children].forEach((element, index) => element.classList.toggle("is-active", index === active));
    const on = active >= 0 ? list.children[active] : null;
    if (on) search.setAttribute("aria-activedescendant", on.id);
    else search.removeAttribute("aria-activedescendant");
    on?.scrollIntoView?.({ block: "nearest" });
  };

  const paintRows = () => {
    rows = menuRows(options, search.value, chosen, { multi, invent });
    rows.forEach((row, index) => { row.at = index; });
    patchList(list, rows, {
      keyOf: (row) => row.key,
      render: (row) => rowHtml(row, id, row.at),
      wire: (element) => {
        if (element.getAttribute("role") !== "option") return;
        element.onmousedown = (event) => event.preventDefault(); // keep the caret in the search box
        element.onclick = () => choose(element.dataset.value);
      },
    });
    nothing.hidden = rows.length > 0;
    if (rows[active]?.kind !== "option") active = firstActive(rows);
    paintActive();
  };

  // ---- the three things a reader does --------------------------------------

  function choose(value) {
    chosen = toggleChoice(chosen, value, { multi });
    // An invented name is on offer from now on, so the row that made it becomes
    // an ordinary row and the query that summoned it has done its job.
    if (invent && !options.some((option) => option.value === value)) search.value = "";
    paintPress();
    clear.disabled = !chosen.length;
    paintRows();
    onChange?.(chosen);
    // A single menu has answered the question the moment it is pressed; a
    // multi one has not, and shutting it after one tick would make ticking
    // three labels three separate openings.
    if (!multi) shut({ focusPress: true });
  }

  function show() {
    if (open) return;
    if (record) {
      focusSearch = true;
      void record.write({ open: true, query: "" });
      return;
    }
    open = true;
    pop.hidden = false;
    press.setAttribute("aria-expanded", "true");
    search.value = "";
    active = -1;
    paintRows();
    search.focus();
    root.ownerDocument.addEventListener("pointerdown", onPointerDown, true);
  }

  function shut({ focusPress = false } = {}) {
    if (!open) return;
    if (record) {
      restorePress = focusPress;
      void record.write({ open: false, query: search.value });
      return;
    }
    open = false;
    pop.hidden = true;
    press.setAttribute("aria-expanded", "false");
    root.ownerDocument.removeEventListener("pointerdown", onPointerDown, true);
    if (focusPress) press.focus();
  }

  const onPointerDown = (event) => {
    if (!root.contains(event.target)) shut();
  };

  // ---- the keyboard --------------------------------------------------------

  /** What each key does inside an open popover. A table rather than a ladder:
   *  the set is closed, and every entry is one line. */
  const KEYS = {
    ArrowDown: () => step(1),
    ArrowUp: () => step(-1),
    Enter: () => {
      if (rows[active]?.kind === "option") choose(rows[active].value);
    },
    Escape: () => shut({ focusPress: true }),
    Tab: () => shut(),
  };

  function step(by) {
    active = moveActive(rows, active, by);
    paintActive();
  }

  search.onkeydown = (event) => {
    const act = KEYS[event.key];
    if (!act) return;
    // Tab is the reader leaving, and taking it would trap them in the popover.
    if (event.key !== "Tab") event.preventDefault();
    act();
  };
  search.oninput = () => {
    if (record) {
      record.schedule({ open: true, query: search.value });
      return;
    }
    active = -1;
    paintRows();
  };

  press.onclick = () => (open ? shut({ focusPress: true }) : show());
  press.onkeydown = (event) => {
    if (event.key !== "ArrowDown") return;
    event.preventDefault();
    show();
  };
  clear.onclick = () => {
    chosen = [];
    paintPress();
    clear.disabled = true;
    paintRows();
    onChange?.(chosen);
    shut({ focusPress: true });
  };

  if (cacheAddress) record = watchUiState(cacheAddress, (saved) => {
    const nextOpen = Boolean(saved?.open);
    open = nextOpen;
    pop.hidden = !nextOpen;
    press.setAttribute("aria-expanded", String(nextOpen));
    search.value = typeof saved?.query === "string" ? saved.query : "";
    active = -1;
    if (nextOpen) {
      paintRows();
      root.ownerDocument.addEventListener("pointerdown", onPointerDown, true);
      if (focusSearch) search.focus();
    } else {
      root.ownerDocument.removeEventListener("pointerdown", onPointerDown, true);
      if (restorePress) press.focus();
    }
    focusSearch = false;
    restorePress = false;
  }, { debounceMs: 180 });

  return {
    element: root,
    /** What is on offer and what is chosen. Never the open state, the query or
     *  the walked row: those are the reader's. */
    update(nextOptions, nextChosen) {
      options = nextOptions || [];
      chosen = [...(nextChosen || [])];
      paintPress();
      clear.disabled = !chosen.length;
      if (open) paintRows();
    },
    isOpen: () => open,
    close: () => shut(),
    dispose: () => {
      if (record) {
        record.dispose();
        root.ownerDocument.removeEventListener("pointerdown", onPointerDown, true);
      } else shut();
    },
  };
}
