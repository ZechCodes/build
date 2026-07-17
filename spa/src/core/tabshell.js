// The unified tab-row helper shared by every worktree-backed surface (task,
// external worktree, primary "main" checkout). DOM-light: it renders and owns
// the `.tabs` row (selection, closable ×, the terminal `+`) — content painting
// stays with the views, which mount into their own `#tabbody`.
//
// Markup reuses the existing `.tabs > .t.active` classes; closable tabs append a
// `<span class="tx">×</span>`; the `+` is a `<div class="t tplus">`.

import { esc } from "./text.js";

/** Pure: the tab-row HTML. `tabs` = [{ id, label, closable? }]; `active` is a
 *  tab id; `hasNewTerminal` renders the trailing `+`; `back` = { title }
 *  renders a leading chevron cell (the surface's way out — up to its project).
 *  Everything is escaped. */
export function tabShellHtml({ tabs, active, hasNewTerminal, back }) {
  const backCell = back ? `<div class="t tback" data-back="1" title="${esc(back.title || "Back")}">‹</div>` : "";
  const cells = (tabs || [])
    .map((tab) => {
      const closer = tab.closable ? `<span class="tx" data-close="${esc(tab.id)}" title="Close terminal">×</span>` : "";
      return `<div class="t ${tab.id === active ? "active" : ""}" data-tab="${esc(tab.id)}">${esc(tab.label)}${closer}</div>`;
    })
    .join("");
  const plus = hasNewTerminal ? `<div class="t tplus" data-newterm="1" title="New terminal">+</div>` : "";
  return `<div class="tabs">${backCell}${cells}${plus}</div>`;
}

/**
 * mountTabShell(host, { tabs, active, onSelect, onClose, onNewTerminal, back, onBack })
 *   → { setActive(id), setTabs(tabs) }
 *
 * Renders the tab row into `host` and wires clicks: a tab body selects (unless
 * the click landed on its ×, which closes), the `+` creates a terminal, and the
 * leading chevron (when `back`/`onBack` are passed) navigates up. The returned
 * controller repaints in place; setTabs preserves the current active id.
 */
export function mountTabShell(host, { tabs, active, onSelect, onClose, onNewTerminal, back, onBack }) {
  let currentTabs = tabs || [];
  let currentActive = active;
  const paint = () => {
    host.innerHTML = tabShellHtml({
      tabs: currentTabs,
      active: currentActive,
      hasNewTerminal: !!onNewTerminal,
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
    if (plus && onNewTerminal) plus.onclick = () => onNewTerminal();
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
