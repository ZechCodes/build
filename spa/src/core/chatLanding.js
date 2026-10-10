// An opening belongs to a visit, not to the transcript DOM: chat switches keep
// that DOM mounted and cache/roster reads can fill it after its first paint.
export const CHAT_LANDING_READ_WAIT_MS = 5000;
const targetArrived = (hasItems, held, next, wasAwaiting, awaiting) =>
  hasItems && ((next !== null && held !== next) || (wasAwaiting && !awaiting));
const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "]);

export function createChatLanding(resumeReporting, repaint = () => {}) {
  let body = null;
  let generation = 0;
  let first = true;
  let target;
  let expectedTop = null;
  let abandoned = false;
  let expired = false;
  let awaiting = true;
  let framePending = false;
  let timer = null;
  let disposed = false;
  let resolved = false;
  let resizeObserver = null;
  const waiting = () => !expired && !abandoned && !resolved && (awaiting || framePending);
  const abandon = () => {
    abandoned = true;
    framePending = false;
    clearTimeout(timer);
    timer = null;
  };
  const input = (event) => {
    if (event.type === "keydown" && !SCROLL_KEYS.has(event.key)) return;
    abandon();
    resumeReporting(body);
  };
  const moved = () => expectedTop !== null && Math.abs(body.scrollTop - expectedTop) >= 1;
  const scroll = () => {
    if (!moved()) return;
    abandon();
    resumeReporting(body);
  };
  const events = ["wheel", "touchstart", "pointerdown", "keydown"];
  const unbind = () => {
    resizeObserver?.disconnect();
    resizeObserver = null;
    if (!body) return;
    for (const type of events) body.removeEventListener(type, input);
    body.removeEventListener("scroll", scroll);
  };
  const reset = () => {
    generation += 1;
    clearTimeout(timer);
    timer = null;
    first = true;
    resolved = false;
    target = undefined;
    expectedTop = null;
    abandoned = false;
    expired = false;
    awaiting = true;
    framePending = false;
  };
  const bind = (scroller) => {
    if (body === scroller) return;
    unbind();
    body = scroller;
    reset();
    for (const type of events) body.addEventListener(type, input, { passive: true });
    body.addEventListener("scroll", scroll, { passive: true });
    if (typeof ResizeObserver === "function") {
      let height = body.clientHeight;
      resizeObserver = new ResizeObserver(() => {
        const visible = height <= 0 && body.clientHeight > 0;
        height = body.clientHeight;
        if (!visible || abandoned || resolved || disposed) return;
        first = true;
        repaint();
      });
      resizeObserver.observe(body);
    }
  };
  const startWait = () => {
    if (timer !== null || expired || abandoned || resolved) return;
    timer = setTimeout(() => {
      timer = null;
      expired = true;
      resumeReporting(body);
    }, CHAT_LANDING_READ_WAIT_MS);
  };
  return {
    reset,
    waiting,
    active: () => !abandoned && !resolved && !disposed,
    painted: () => { if (body) expectedTop = body.scrollTop; },
    prepare(scroller, { hasItems, target: nextTarget, waitingForHistory }) {
      bind(scroller);
      if (moved()) abandon();
      startWait();
      const wasAwaiting = awaiting;
      awaiting = waitingForHistory || body.clientHeight <= 0;
      const changedTarget = targetArrived(hasItems, target, nextTarget, wasAwaiting, awaiting);
      const opening = !abandoned && !resolved && (first || changedTarget);
      if (hasItems) {
        first = false;
        target = nextTarget;
      }
      if (opening) generation += 1;
      const ticket = generation;
      return {
        opening,
        canLand: () => {
          if (moved()) abandon();
          return !disposed && !abandoned && generation === ticket;
        },
        onLand: (settled) => {
          if (disposed || generation !== ticket) return;
          expectedTop = body.scrollTop;
          framePending = !settled;
          if (settled && hasItems && !awaiting) {
            resolved = true;
            clearTimeout(timer);
            timer = null;
          }
          if (settled && !waiting()) resumeReporting(body);
        },
      };
    },
    dispose() {
      disposed = true;
      reset();
      unbind();
    },
  };
}
