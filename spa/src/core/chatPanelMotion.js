// The conversation panel's dock/card change has two movements. First the work
// gets or gives back the column's room while the live panel stays put. Then
// that same panel moves into its new geometry. Keeping this here leaves the
// rail's state and painting code concerned only with what is open and pinned.

import { isAtBottom } from "./paintKeepingPlace.js";

export const CHAT_LAYOUT_TRANSITION_MS = 240;
export const CHAT_PANEL_TRANSITION_MS = 160;
const EASING = "cubic-bezier(.2,.8,.2,1)";
const INLINE_GEOMETRY = ["position", "left", "top", "width", "height", "max-height", "margin", "z-index"];

const usableRect = (rect) => !!rect?.width && !!rect?.height;

function pinPanel(panel, rect) {
  Object.assign(panel.style, {
    position: "fixed",
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
    maxHeight: "none",
    margin: "0",
    zIndex: "38",
  });
}

function releasePanel(panel) {
  for (const property of INLINE_GEOMETRY) panel.style.removeProperty(property);
}

/** Hold a reader's place while the panel changes height. At the bottom, the
 * bottom remains the anchor; in history, the exact scrollTop remains the
 * anchor. ResizeObserver runs before paint in browsers, so the bottom does not
 * visibly drift during the second movement. */
function holdScrollPlace(scroller) {
  if (!scroller) return { settle() {}, dispose() {} };
  const top = scroller.scrollTop;
  const bottom = isAtBottom(scroller);
  let holding = true;
  const settle = () => {
    if (!holding) return;
    scroller.scrollTop = bottom ? scroller.scrollHeight : top;
  };
  const observer = typeof ResizeObserver === "function" ? new ResizeObserver(settle) : null;
  observer?.observe(scroller);
  const releaseForInput = () => {
    holding = false;
    observer?.disconnect();
  };
  for (const type of ["wheel", "touchstart", "pointerdown", "keydown"]) {
    scroller.addEventListener(type, releaseForInput, { once: true });
  }
  return {
    settle,
    dispose() {
      observer?.disconnect();
      for (const type of ["wheel", "touchstart", "pointerdown", "keydown"]) {
        scroller.removeEventListener(type, releaseForInput);
      }
    },
  };
}

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

/** One controller per mounted rail. `apply` changes pin state and paints it;
 * `direction` exists only to let the rail coordinate its card chrome/scrim. */
export function createChatPanelMotion(host, { onPhase = () => {} } = {}) {
  let generation = 0;
  let cleanup = null;

  const setPhase = (direction, phase) => {
    if (direction) {
      host.dataset.panelTransition = direction;
      host.dataset.panelTransitionPhase = phase;
    } else {
      delete host.dataset.panelTransition;
      delete host.dataset.panelTransitionPhase;
    }
    onPhase(phase);
  };

  const cancel = () => {
    generation += 1;
    cleanup?.();
    cleanup = null;
    setPhase(null, "idle");
  };

  const run = ({ panel, direction, apply, scroller = null }) => {
    // During a rapid reversal getBoundingClientRect is the on-screen animated
    // rectangle. Capture it before cancelling the previous run and use it as
    // the new fixed starting point.
    const before = panel?.getBoundingClientRect();
    cancel();
    if (!panel) {
      apply();
      return;
    }
    const scrollPlace = holdScrollPlace(scroller);
    if (reducedMotion() || !usableRect(before)) {
      apply();
      scrollPlace.settle();
      scrollPlace.dispose();
      return;
    }

    const current = ++generation;
    let animation = null;
    let layoutTimer = null;
    let finished = false;

    const finish = () => {
      if (finished || current !== generation) return;
      finished = true;
      clearTimeout(layoutTimer);
      releasePanel(panel);
      scrollPlace.settle();
      scrollPlace.dispose();
      cleanup = null;
      setPhase(null, "idle");
    };

    pinPanel(panel, before);
    setPhase(direction, "layout");
    apply();
    layoutTimer = setTimeout(() => {
      if (current !== generation) return;
      // The reserved width has now settled. Release only for this synchronous
      // measurement; pinning it again in the same task prevents a painted jump.
      releasePanel(panel);
      const after = panel.getBoundingClientRect();
      if (!usableRect(after)) return finish();
      pinPanel(panel, before);
      setPhase(direction, "panel");
      animation = panel.animate?.(
        [
          { left: `${before.left}px`, top: `${before.top}px`, width: `${before.width}px`, height: `${before.height}px` },
          { left: `${after.left}px`, top: `${after.top}px`, width: `${after.width}px`, height: `${after.height}px` },
        ],
        { duration: CHAT_PANEL_TRANSITION_MS, easing: EASING },
      );
      if (!animation) return finish();
      pinPanel(panel, after);
      animation.onfinish = finish;
      animation.oncancel = finish;
    }, CHAT_LAYOUT_TRANSITION_MS);

    cleanup = () => {
      clearTimeout(layoutTimer);
      animation?.cancel();
      releasePanel(panel);
      scrollPlace.settle();
      scrollPlace.dispose();
    };
  };

  return { run, cancel };
}
