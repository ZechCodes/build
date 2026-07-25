// The unified tab-row helper shared by every worktree-backed surface (task,
// external worktree, primary "main" checkout). DOM-light: it renders and owns
// the `.tabs` row (selection, closable ×, the new-tab `+`) — content painting
// stays with the views, which mount into their own `#tabbody`.
//
// Markup reuses the existing `.tabs > .t.active` classes; closable tabs append a
// `<span class="tx">×</span>`; the `+` is a `<div class="t tplus">`.
//
// The `+` is a menu, not a single verb: a new tab can be a shell, or a coding
// agent the human drives (see NEW_TAB_KINDS in surfaceTabs.js). Its menu is
// mounted on `document.body` and positioned from the `+`'s box, because the row
// itself scrolls horizontally and would clip a child popup.

import { esc } from "./text.js";

/** Pure: the tab-row HTML. `tabs` = [{ id, label, closable? }]; `active` is a
 *  tab id; a non-empty `newTabOptions` renders the trailing `+`; `back` =
 *  { title } renders a leading chevron cell (the surface's way out — up to its
 *  project). Everything is escaped. */
export function tabShellHtml({ tabs, active, newTabOptions, back }) {
  const backCell = back
    ? `<div class="t tback" data-back="1" title="${esc(back.title || "Back")}" aria-label="${esc(back.title || "Back")}">‹</div>`
    : "";
  const cells = (tabs || [])
    .map((tab) => {
      const closer = tab.closable ? `<span class="tx" data-close="${esc(tab.id)}" title="Close tab">×</span>` : "";
      // `title` is where a tab keeps what its label leaves out — an Agent tab's
      // provider, say. Absent for tabs whose label is the whole story.
      const hover = tab.title ? ` title="${esc(tab.title)}"` : "";
      return `<div class="t ${tab.id === active ? "active" : ""}" data-tab="${esc(tab.id)}"${hover}>${esc(tab.label)}${closer}</div>`;
    })
    .join("");
  const plus = (newTabOptions || []).length
    ? `<div class="t tplus" data-newtab="1" title="New tab" aria-haspopup="menu">+</div>`
    : "";
  return `<div class="tabs">${backCell}${cells}${plus}</div>`;
}

/** Pure: the `+` menu's items — one per new-tab kind, each carrying its kind id.
 *  Same item shape as the split-button menu (`.mi > .mt + .md`), so the two read
 *  as one component family. */
export function newTabMenuHtml(options) {
  return (options || [])
    .map(
      (option) =>
        `<div class="mi" data-kind="${esc(option.id)}"><span class="mt">${esc(option.label)}</span><span class="md">${esc(option.description || "")}</span></div>`,
    )
    .join("");
}

/**
 * mountTabShell(host, { tabs, active, newTabOptions, onSelect, onClose, onNewTab, back, onBack })
 *   → { setActive(id), setTabs(tabs) }
 *
 * Renders the tab row into `host` and wires clicks: a tab body selects (unless
 * the click landed on its ×, which closes), the `+` opens the new-tab menu and
 * reports the chosen kind through `onNewTab(kind)`, and the leading chevron
 * (when `back`/`onBack` are passed) navigates up. The returned controller
 * repaints in place; setTabs preserves the current active id. Any open menu
 * closes on a repaint, on Escape, on a pointerdown outside it, and on a pick —
 * it lives on document.body, so nothing else would collect it.
 */
export function mountTabShell(host, { tabs, active, newTabOptions, onSelect, onClose, onNewTab, back, onBack }) {
  let currentTabs = tabs || [];
  let currentActive = active;
  let openMenu = null; // { element, dismiss() } while the + menu is up

  const closeMenu = () => {
    if (!openMenu) return;
    openMenu.dismiss();
    openMenu.element.remove();
    openMenu = null;
  };

  const openNewTabMenu = (anchor) => {
    const element = document.createElement("div");
    element.className = "tabmenu";
    element.setAttribute("role", "menu");
    element.innerHTML = newTabMenuHtml(newTabOptions);
    // Fixed to the viewport, aligned under the `+`: the row scrolls, so a child
    // popup would be clipped by its own container.
    const box = anchor.getBoundingClientRect ? anchor.getBoundingClientRect() : { left: 0, bottom: 0 };
    element.style.position = "fixed";
    element.style.left = `${box.left}px`;
    element.style.top = `${box.bottom + 4}px`;
    document.body.appendChild(element);

    const onKeydown = (event) => {
      if (event.key === "Escape") closeMenu();
    };
    // Live immediately, no arming tick: this menu opens on `click`, which always
    // follows its own `pointerdown`, so the gesture that opened it cannot reach
    // this listener. (Deferring the listener instead loses the race against a
    // fast real pointer — a trusted click can land before a setTimeout(0) runs.)
    // A pointerdown on the `+` is left to the `+`, which toggles itself closed.
    const onOutside = (event) => {
      if (element.contains(event.target)) return;
      if (anchor.contains && anchor.contains(event.target)) return;
      closeMenu();
    };
    document.addEventListener("keydown", onKeydown);
    document.addEventListener("pointerdown", onOutside);
    openMenu = {
      element,
      dismiss() {
        document.removeEventListener("keydown", onKeydown);
        document.removeEventListener("pointerdown", onOutside);
      },
    };

    element.querySelectorAll(".mi").forEach((item) => {
      item.onclick = () => {
        const kind = item.dataset.kind;
        closeMenu();
        onNewTab(kind);
      };
    });
  };

  const paint = () => {
    closeMenu();
    host.innerHTML = tabShellHtml({
      tabs: currentTabs,
      active: currentActive,
      newTabOptions: onNewTab ? newTabOptions : [],
      back: onBack ? back || { title: "Back" } : null,
    });
    const backCell = host.querySelector(".tback");
    if (backCell && onBack) backCell.onclick = () => onBack();
    host.querySelectorAll(".t[data-tab]").forEach((cell) => {
      cell.onclick = (event) => {
        const closeId = event.target && event.target.dataset ? event.target.dataset.close : null;
        if (closeId) {
          if (onClose) onClose(closeId);
          return;
        }
        if (onSelect) onSelect(cell.dataset.tab);
      };
    });
    const plus = host.querySelector(".tplus");
    if (plus && onNewTab) {
      plus.onclick = (event) => {
        event.stopPropagation();
        if (openMenu) closeMenu();
        else openNewTabMenu(plus);
      };
    }
  };
  paint();
  return {
    setActive(id) {
      currentActive = id;
      paint();
    },
    setTabs(next) {
      currentTabs = next || [];
      paint();
    },
  };
}
