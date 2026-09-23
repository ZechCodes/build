// A floating jump control for any scroll surface with an unread divider.
// The owner supplies the selector; the dock has zero height and cannot move
// timeline content. Chat can adopt this without owning issue page markup.
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
  const jump = () => {
    const line = target();
    if (!line) return;
    jumping = true;
    if (dock) dock.hidden = true;
    line.scrollIntoView({ behavior: "smooth", block: "start" });
  };
  const sync = () => {
    if (!target()) { dock = null; return; }
    dock = document.createElement("div");
    dock.className = "new-messages-dock";
    dock.innerHTML = '<button class="new-messages-pill" type="button" aria-label="Jump to first unread activity">New messages</button>';
    dock.querySelector("button").addEventListener("click", jump);
    scroller.append(dock);
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
      dock?.remove();
    },
  };
}
