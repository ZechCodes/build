import { mountLongPress } from "./longPress.js";
import { menuButtonMarkup } from "./splitButton.js";

const rowOf = (event) => event.target.closest?.('.frow[data-path], .frow[data-root-head], .frow[data-upload-root]');
const interactive = (event) => event.target.closest?.('button, input, .splitmenu, .fupload-folder');
const clamp = (value, extent, bound) => Math.max(8, Math.min(value, bound - extent - 8));

/** A delegated menu for cache-rendered rows. The menu lives above the scrolling
 * tree, so replacing a listing cannot replace its controls mid-gesture. */
export function mountFileContextMenu(host, { itemsFor, onChoose }) {
  const document = host.ownerDocument;
  let popup = null;
  let owner = null;
  const close = (restoreFocus = false) => {
    popup?.remove();
    popup = null;
    if (restoreFocus && owner?.isConnected) owner.focus({ preventScroll: true });
    owner = null;
  };
  const choose = (item) => {
    const row = owner;
    const action = item.dataset.action;
    close(true);
    onChoose(action, row);
  };
  const onMenuKey = (event) => {
    const items = [...popup.querySelectorAll('[role="menuitem"]')];
    const index = items.indexOf(document.activeElement);
    const moves = { ArrowDown: (index + 1) % items.length, ArrowUp: (index + items.length - 1) % items.length, Home: 0, End: items.length - 1 };
    event.stopPropagation();
    if (Object.hasOwn(moves, event.key)) { event.preventDefault(); items[moves[event.key]].focus(); }
    if (event.key === "Enter" || event.key === " ") { event.preventDefault(); choose(document.activeElement); }
    if (event.key === "Tab") close();
  };
  const show = (row, point = null) => {
    close();
    const items = itemsFor(row);
    if (!items.length) return false;
    owner = row;
    const wrapper = document.createElement("div");
    wrapper.innerHTML = menuButtonMarkup("", items, { title: "File actions" });
    popup = wrapper.querySelector(".splitmenu");
    popup.classList.add("file-context-menu");
    popup.hidden = false;
    document.body.append(popup);
    const box = row.getBoundingClientRect();
    const size = popup.getBoundingClientRect();
    popup.style.left = `${clamp(point?.x ?? box.left, size.width, window.innerWidth)}px`;
    popup.style.top = `${clamp(point?.y ?? box.bottom, size.height, window.innerHeight)}px`;
    popup.addEventListener("keydown", onMenuKey);
    popup.addEventListener("click", (event) => {
      const item = event.target.closest('[role="menuitem"]');
      if (item) choose(item);
    });
    popup.querySelector('[role="menuitem"]').focus({ preventScroll: true });
    return true;
  };
  const onContextMenu = (event) => {
    const row = rowOf(event);
    if (!row || interactive(event)) return;
    const point = event.clientX || event.clientY ? { x: event.clientX, y: event.clientY } : null;
    if (show(row, point)) { event.preventDefault(); event.stopPropagation(); }
  };
  const onHostKey = (event) => {
    if (event.key !== "ContextMenu" && !(event.key === "F10" && event.shiftKey)) return;
    const row = rowOf(event);
    if (row && !interactive(event) && show(row)) { event.preventDefault(); event.stopPropagation(); }
  };
  const onOutside = (event) => { if (popup && !popup.contains(event.target)) close(); };
  const onEscape = (event) => {
    if (popup && event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(true); }
  };
  const onScroll = (event) => { if (popup && !popup.contains(event.target)) close(); };
  const longPress = mountLongPress(host, {
    targetOf: rowOf, canStart: (event) => !interactive(event),
    onReveal: (row) => { row.classList.add("fupload-revealed"); show(row); },
    onDismiss: (row) => row.classList.remove("fupload-revealed"),
  });
  const observer = new MutationObserver(() => { if (owner && !host.contains(owner)) close(); });
  observer.observe(host, { childList: true, subtree: true });
  host.addEventListener("contextmenu", onContextMenu);
  host.addEventListener("keydown", onHostKey, true);
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("click", onOutside, true);
  document.addEventListener("keydown", onEscape, true);
  document.addEventListener("scroll", onScroll, true);
  window.addEventListener("resize", close);
  return {
    close,
    reset() { close(); longPress.clear(); },
    dispose() {
      close(); longPress.dispose(); observer.disconnect();
      host.removeEventListener("contextmenu", onContextMenu);
      host.removeEventListener("keydown", onHostKey, true);
      document.removeEventListener("pointerdown", onOutside, true);
      document.removeEventListener("click", onOutside, true);
      document.removeEventListener("keydown", onEscape, true);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
    },
  };
}
