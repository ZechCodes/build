// Touch → wheel gesture state machine for terminal panes. ghostty-web 0.4.0
// registers only mouse/wheel listeners, so mountTerminalPane translates
// one-finger drags into pixel-mode WheelEvents dispatched at the pane host,
// reusing ghostty's own handleWheel (scrollback on the normal screen, arrow-key
// escapes on the alternate screen). DOM-free: all effects are injected so the
// gesture rules are unit-testable in vitest's node environment.

const SLOP_PX = 8; // movement below this stays a tap: no emission, no preventDefault
const ALT_ARROW_PX = 33; // ghostty's alt-screen quantum: round(deltaY / 33) arrows, ≤5 per event
const ALT_MAX_ARROWS_PER_EVENT = 5;
const VELOCITY_WINDOW_MS = 100; // release velocity is sampled over the last moves in this window
const MOMENTUM_START_PX_PER_MS = 0.05;
const MOMENTUM_STOP_PX_PER_MS = 0.01;
const MOMENTUM_HALF_LIFE_MS = 300;

/**
 * createTouchScroll({ dispatchWheel, requestFrame, cancelFrame }) →
 *   { onTouchStart, onTouchMove, onTouchEnd, onTouchCancel, dispose }
 *
 * dispatchWheel(deltaYPx) — effect: deliver a pixel-mode wheel delta
 *   (finger moving up ⇒ positive deltaY ⇒ scroll down, matching native touch).
 * requestFrame(cb)        — rAF injection; cb receives a timestamp (ms).
 * cancelFrame(id)         — cancel a pending frame.
 *
 * Handlers read only touches/changedTouches ({ identifier, clientY }) and
 * event.timeStamp, and call preventDefault()/stopPropagation() once a scroll
 * is underway (onTouchEnd expects to run capture-phase, before ghostty's own
 * canvas touchend focus handler).
 */
/**
 * createWheelQuantizer({ isAltScreen, getCellHeight, emit }) → dispatch(deltaYPx)
 *
 * ghostty's alt-screen wheel handler quantizes each event in isolation —
 * `Math.round(deltaY / 33)` arrows, capped at 5, remainder discarded — so the
 * small per-move deltas of a slow touch drag would all round to zero. On the
 * alternate screen this accumulates deltas and emits one 33px-multiple wheel
 * event per *cell height* crossed (a drag moves the same number of lines as it
 * would on the normal screen); normal-screen deltas pass through untouched.
 */
export function createWheelQuantizer({ isAltScreen, getCellHeight, emit }) {
  let bankedDelta = 0;
  return (deltaY) => {
    if (!isAltScreen()) {
      bankedDelta = 0;
      emit(deltaY);
      return;
    }
    bankedDelta += deltaY;
    const cellHeight = getCellHeight();
    const owed = Math.trunc(bankedDelta / cellHeight);
    if (owed === 0) return;
    const arrows = Math.max(-ALT_MAX_ARROWS_PER_EVENT, Math.min(ALT_MAX_ARROWS_PER_EVENT, owed));
    bankedDelta -= arrows * cellHeight;
    emit(arrows * ALT_ARROW_PX);
  };
}

export function createTouchScroll({ dispatchWheel, requestFrame, cancelFrame }) {
  let tracked = null; // { id, lastY, bankedDelta, pastSlop, samples: [{ t, y }] }
  let suppressed = false; // a second finger landed: ignore moves until all fingers lift
  let frameId = null; // pending momentum frame, when a flick is coasting
  let velocity = 0; // px/ms, in wheel-delta sign
  let lastFrameTime = null;

  const stopMomentum = () => {
    if (frameId !== null) { cancelFrame(frameId); frameId = null; }
    velocity = 0;
    lastFrameTime = null;
  };

  // First frame only records its timestamp: event timeStamps and frame
  // timestamps share an origin in browsers, but not necessarily in fakes.
  const momentumStep = (now) => {
    frameId = null;
    if (lastFrameTime !== null) {
      const dt = now - lastFrameTime;
      if (dt > 0) {
        dispatchWheel(velocity * dt);
        velocity *= Math.pow(0.5, dt / MOMENTUM_HALF_LIFE_MS);
      }
      if (Math.abs(velocity) < MOMENTUM_STOP_PX_PER_MS) { stopMomentum(); return; }
    }
    lastFrameTime = now;
    frameId = requestFrame(momentumStep);
  };

  const findTouch = (touchList, id) => {
    for (const t of touchList) if (t.identifier === id) return t;
    return null;
  };

  return {
    onTouchStart(event) {
      stopMomentum();
      if (event.touches.length > 1) { // second concurrent finger: end the gesture
        tracked = null;
        suppressed = true;
        return;
      }
      if (suppressed) return;
      const touch = event.changedTouches[0];
      tracked = {
        id: touch.identifier,
        lastY: touch.clientY,
        bankedDelta: 0,
        pastSlop: false,
        samples: [{ t: event.timeStamp, y: touch.clientY }],
      };
    },

    onTouchMove(event) {
      if (!tracked) return;
      const touch = findTouch(event.changedTouches, tracked.id);
      if (!touch) return;
      const delta = tracked.lastY - touch.clientY;
      tracked.lastY = touch.clientY;
      tracked.samples.push({ t: event.timeStamp, y: touch.clientY });
      if (!tracked.pastSlop) {
        tracked.bankedDelta += delta;
        if (Math.abs(tracked.bankedDelta) <= SLOP_PX) return; // still a possible tap
        tracked.pastSlop = true;
        event.preventDefault();
        dispatchWheel(tracked.bankedDelta);
        tracked.bankedDelta = 0;
        return;
      }
      event.preventDefault();
      if (delta !== 0) dispatchWheel(delta);
    },

    onTouchEnd(event) {
      const wasSuppressed = suppressed;
      if (event.touches.length === 0) suppressed = false;
      if (!tracked) {
        // A multi-touch gesture ending is not a tap: keep ghostty's canvas
        // touchend (textarea focus ⇒ mobile keyboard) from firing for it.
        if (wasSuppressed) event.stopPropagation();
        return;
      }
      const touch = findTouch(event.changedTouches, tracked.id);
      if (!touch) return;
      const { pastSlop, samples } = tracked;
      tracked = null;
      if (!pastSlop) return; // a tap — let ghostty's touchend focus follow
      event.stopPropagation(); // a drag — don't pop the keyboard after scrolling
      if (event.cancelable) event.preventDefault(); // and suppress the synthesized click
      const endTime = event.timeStamp;
      const anchor = samples.find((s) => s.t >= endTime - VELOCITY_WINDOW_MS);
      if (!anchor || endTime <= anchor.t) return; // finger paused before lifting
      const releaseVelocity = (anchor.y - touch.clientY) / (endTime - anchor.t);
      if (Math.abs(releaseVelocity) < MOMENTUM_START_PX_PER_MS) return;
      velocity = releaseVelocity;
      lastFrameTime = null;
      frameId = requestFrame(momentumStep);
    },

    onTouchCancel() {
      stopMomentum();
      tracked = null;
      suppressed = false;
    },

    dispose() {
      stopMomentum();
      tracked = null;
      suppressed = false;
    },
  };
}
