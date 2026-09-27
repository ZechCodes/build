// A floating jump control for any scroll surface with an unread divider.
// The owner supplies the selector; the dock has zero height and cannot move
// timeline content. Chat can adopt this without owning task page markup.
import { scrollWithin } from "./scrollWithin.js";

export function mountNewMessagesPill(scroller, { targetSelector }) {
  let jumping = false;
  let dock = null;
  const target = () => scroller.querySelector(targetSelector);
  const update = () => {
    const line = target();
    if (!dock?.isConnected || !line) return;
    const below = line.getBoundingClientRect().top >= scroller.getBoundingClientRect().bottom;
    dock.hidden = jumping || !below;
    if (!below) jumping = false;
  };
  const resizeObserver = typeof ResizeObserver === "function" ? new ResizeObserver(update) : null;
  const observeLayout = () => {
    resizeObserver?.disconnect();
    resizeObserver?.observe(scroller);
    // A panel can change its content's height without the browser window or
    // scroller changing size (for example when comments wrap in a narrow rail).
    const content = [...scroller.children].find((child) => child !== dock);
    if (content) resizeObserver?.observe(content);
  };
  const jump = () => {
    const line = target();
    if (!line) return;
    jumping = true;
    if (dock) dock.hidden = true;
    scrollWithin(scroller, line);
  };
  // Safe to call again over a dock still standing: a surface that repaints
  // only part of itself keeps the scroller's children, and with them the dock.
  const sync = () => {
    dock?.remove();
    if (!target()) { dock = null; resizeObserver?.disconnect(); return; }
    dock = document.createElement("div");
    dock.className = "new-messages-dock";
    dock.innerHTML = '<button class="new-messages-pill" type="button" aria-label="Jump to first unread activity">New messages</button>';
    dock.querySelector("button").addEventListener("click", jump);
    scroller.append(dock);
    observeLayout();
    update();
  };
  scroller.addEventListener("scroll", update, { passive: true });
  window.addEventListener("resize", update);
  return {
    sync,
    update,
    dispose() {
      scroller.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      resizeObserver?.disconnect();
      dock?.remove();
    },
  };
}
