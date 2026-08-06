// The unified tab-row helper shared by every worktree-backed surface (task,
// external worktree, primary "main" checkout, plan). DOM-light: it renders and
// owns the `.tabs` row (selection, closable ×, the new-tab `+`, the trailing
// right cluster) — content painting stays with the views, which mount into
// their own `#tabbody`.
//
// Markup reuses the existing `.tabs > .t.active` classes; closable tabs append a
// `<span class="tx">×</span>`; the `+` is a `<div class="t tplus">`.
//
// The right cluster is where the surface's project-wide entries live: icon tabs
// (`rightTabs`, selectable like any other tab) and a `⋯` menu whose items are
// actions the surface routes (`onMenuPick`). Every project surface passes the
// same cluster, so Inbox and the menu sit in one place wherever the user is.
//
// Both popups (the `+`'s kinds and the `⋯`'s actions) are one implementation,
// mounted on `document.body` and positioned from their anchor's box, because the
// row itself scrolls horizontally and would clip a child popup.

import { esc } from "./text.js";

/** Pure: one popup item per entry, carrying its id under `data-<key>`. Items are
 *  buttons — they perform an action, and the keyboard must reach them. Same
 *  `.mi > .mt + .md` shape as the split-button menu, so the menus read as one
 *  component family. */
function menuItemsHtml(items, key) {
  return (items || [])
    .map(
      (item) =>
        `<button class="mi" data-${key}="${esc(item.id)}" type="button" role="menuitem"><span class="mt">${esc(item.label)}</span><span class="md">${esc(item.description || "")}</span></button>`,
    )
    .join("");
}

/** Pure: the tab-row HTML. `tabs` = [{ id, label, closable? }]; `active` is a
 *  tab id; a non-empty `newTabOptions` renders the trailing `+`; `back` =
 *  { title } renders a leading chevron cell (the surface's way out — up to its
 *  project); `rightTabs` = [{ id, glyph, label }] and a non-empty `menu` render
 *  the trailing right cluster. Everything is escaped. */
export function tabShellHtml({ tabs, active, newTabOptions, back, rightTabs, menu }) {
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
  // An icon tab IS a tab: same `data-tab`, same active class, same onSelect. Its
  // label is the tooltip and the accessible name — the glyph is the cell.
  const iconCells = (rightTabs || [])
    .map(
      (tab) =>
        `<div class="t ticon ${tab.id === active ? "active" : ""}" data-tab="${esc(tab.id)}" title="${esc(tab.label)}" aria-label="${esc(tab.label)}">${esc(tab.glyph)}</div>`,
    )
    .join("");
  const dots = (menu || []).length
    ? `<div class="t tmenu" data-menu="1" title="More" aria-label="More actions" aria-haspopup="menu">⋯</div>`
    : "";
  const cluster = iconCells || dots ? `<div class="tabs-right">${iconCells}${dots}</div>` : "";
  return `<div class="tabs">${backCell}${cells}${plus}${cluster}</div>`;
}

/** Pure: the `+` menu's items — one per new-tab kind, each carrying its kind id. */
export function newTabMenuHtml(options) {
  return menuItemsHtml(options, "kind");
}

/** Pure: the `⋯` menu's items — one per surface action, each carrying its id. */
export function surfaceMenuHtml(items) {
  return menuItemsHtml(items, "action");
}

/**
 * mountTabShell(host, { tabs, active, newTabOptions, onSelect, onClose, onNewTab,
 *                       back, onBack, rightTabs, menu, onMenuPick })
 *   → { setActive(id), setTabs(tabs) }
 *
 * Renders the tab row into `host` and wires clicks: a tab body selects (unless
 * the click landed on its ×, which closes), the `+` opens the new-tab menu and
 * reports the chosen kind through `onNewTab(kind)`, the `⋯` opens the surface
 * menu and reports the chosen action through `onMenuPick(action)`, and the
 * leading chevron (when `back`/`onBack` are passed) navigates up. Icon tabs in
 * the right cluster select through the same `onSelect` as any other tab. The
 * returned controller repaints in place; setTabs preserves the current active
 * id. Any open menu closes on a repaint, on Escape, on a pointerdown outside it,
 * and on a pick — it lives on document.body, so nothing else would collect it.
 */
export function mountTabShell(
  host,
  { tabs, active, newTabOptions, onSelect, onClose, onNewTab, back, onBack, rightTabs, menu, onMenuPick },
) {
  let currentTabs = tabs || [];
  let currentActive = active;
  let openMenu = null; // { element, dismiss() } while a popup is up

  const closeMenu = () => {
    if (!openMenu) return;
    openMenu.dismiss();
    openMenu.element.remove();
    openMenu = null;
  };

  // One popup for both the `+` and the `⋯`: the items' HTML, the attribute their
  // ids ride on, and what to do with a pick. `alignRight` hangs the popup off the
  // anchor's right edge, which is where a right-cluster control's menu belongs.
  const openPopup = (anchor, itemsHtml, key, onPick, { alignRight = false } = {}) => {
    const element = document.createElement("div");
    element.className = "tabmenu";
    element.setAttribute("role", "menu");
    element.innerHTML = itemsHtml;
    // Fixed to the viewport, aligned under its anchor: the row scrolls, so a
    // child popup would be clipped by its own container.
    const box = anchor.getBoundingClientRect ? anchor.getBoundingClientRect() : { left: 0, right: 0, bottom: 0 };
    element.style.position = "fixed";
    if (alignRight) element.style.right = `${Math.max(0, (window.innerWidth || 0) - box.right)}px`;
    else element.style.left = `${box.left}px`;
    element.style.top = `${box.bottom + 4}px`;
    document.body.appendChild(element);

    const onKeydown = (event) => {
      if (event.key === "Escape") closeMenu();
    };
    // Live immediately, no arming tick: this menu opens on `click`, which always
    // follows its own `pointerdown`, so the gesture that opened it cannot reach
    // this listener. (Deferring the listener instead loses the race against a
    // fast real pointer — a trusted click can land before a setTimeout(0) runs.)
    // A pointerdown on the anchor is left to the anchor, which toggles itself closed.
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
        const id = item.dataset[key];
        closeMenu();
        onPick(id);
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
      rightTabs,
      menu: onMenuPick ? menu : [],
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
        // A dropdown is how a control offers a choice; with one thing to open,
        // the `+` IS that thing. (A worktree's agent is not on this menu — it
        // has its own permanent tab — so today the only kind is the shell.)
        if ((newTabOptions || []).length < 2) {
          closeMenu();
          onNewTab(newTabOptions && newTabOptions[0] ? newTabOptions[0].id : "shell");
          return;
        }
        if (openMenu) closeMenu();
        else openPopup(plus, newTabMenuHtml(newTabOptions), "kind", (kind) => onNewTab(kind));
      };
    }
    const dots = host.querySelector(".tmenu");
    if (dots && onMenuPick) {
      dots.onclick = (event) => {
        event.stopPropagation();
        if (openMenu) closeMenu();
        else openPopup(dots, surfaceMenuHtml(menu), "action", (action) => onMenuPick(action), { alignRight: true });
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
