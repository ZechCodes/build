// The touch key bar: a row of the keys a phone's soft keyboard does not have,
// pinned to the bottom of a terminal pane. Rules live in touchKeys.js; this is
// the DOM — render, press, paint the sticky modifier states, and stay above the
// on-screen keyboard.

import { TOUCH_KEYS, keySequence, keyboardInset } from "./touchKeys.js";

/**
 * Whether this is a device the bar is for. `(pointer: coarse)` is the real
 * question — a finger, not a mouse — with maxTouchPoints as the fallback for
 * browsers that answer the query but not truthfully (and for test windows).
 */
export function coarsePointer(win = window) {
  if (win.matchMedia) {
    try {
      if (win.matchMedia("(pointer: coarse)").matches) return true;
    } catch {
      /* a window whose matchMedia cannot answer this query */
    }
  }
  return (win.navigator?.maxTouchPoints ?? 0) > 0;
}

/**
 * mountKeyBar(host, { send, sticky, applicationCursor }) → { element, dispose }
 *   send    — (data) => void: the bytes for a pressed key, straight to the PTY.
 *   sticky  — the createStickyModifiers() latch SHARED with the pane, so a
 *             modifier armed here also folds into the next soft-keyboard press.
 *   applicationCursor — () => bool: the terminal's DECCKM state.
 *
 * Presses are handled on pointerdown with preventDefault, never on click: the
 * default action of touching a button is to move focus, and moving focus off
 * ghostty's hidden textarea dismisses the soft keyboard — so every arrow tap
 * would close the keyboard the bar exists to supplement.
 */
export function mountKeyBar(host, { send, sticky, applicationCursor = () => false, win = window }) {
  const bar = document.createElement("div");
  bar.className = "termkeys";
  bar.setAttribute("role", "toolbar");
  bar.setAttribute("aria-label", "Terminal keys");
  for (const key of TOUCH_KEYS) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = key.modifier ? "termkey termkey-mod" : "termkey";
    button.dataset.key = key.id;
    button.title = key.title || key.label;
    button.textContent = key.label;
    if (key.modifier) button.setAttribute("aria-pressed", "false");
    bar.appendChild(button);
  }
  host.appendChild(bar);

  const paint = () => {
    for (const key of TOUCH_KEYS) {
      if (!key.modifier) continue;
      const button = bar.querySelector(`[data-key="${key.id}"]`);
      const state = sticky.state(key.modifier);
      button.classList.toggle("armed", state === "armed");
      button.classList.toggle("locked", state === "locked");
      button.setAttribute("aria-pressed", state === "off" ? "false" : "true");
    }
  };

  const onPointerDown = (event) => {
    const button = event.target.closest?.(".termkey");
    if (!button) return;
    event.preventDefault(); // keep focus — and the soft keyboard — where it is
    const key = TOUCH_KEYS.find((k) => k.id === button.dataset.key);
    if (!key) return;
    if (key.modifier) {
      sticky.press(key.modifier);
      return;
    }
    const data = keySequence(key.id, { ...sticky.pressed(), applicationCursor: applicationCursor() });
    if (data === null) return;
    send(data);
    sticky.consume();
  };
  bar.addEventListener("pointerdown", onPointerDown);
  // Belt and braces on the same focus concern: where a browser fires mousedown
  // anyway (pointerdown's prevented default is supposed to suppress it), moving
  // focus is ITS default action too, and that alone would drop the keyboard.
  const onMouseDown = (event) => {
    if (event.target.closest?.(".termkey")) event.preventDefault();
  };
  bar.addEventListener("mousedown", onMouseDown);

  // Lift the bar clear of an on-screen keyboard the layout viewport ignored
  // (iOS Safari): the pane's own bottom is underneath it, so without this the
  // bar is only visible when the keyboard is down.
  const lift = () => {
    const inset = keyboardInset(win);
    bar.style.transform = inset > 0 ? `translateY(-${inset}px)` : "";
  };
  lift();
  const viewport = win.visualViewport;
  viewport?.addEventListener("resize", lift);
  viewport?.addEventListener("scroll", lift);

  // The bar paints the latch it shares with the pane, so a modifier the PANE
  // consumed (a soft-keyboard press) un-highlights here too.
  const stopPainting = sticky.watch(paint);
  paint();

  return {
    element: bar,
    dispose() {
      stopPainting();
      bar.removeEventListener("pointerdown", onPointerDown);
      bar.removeEventListener("mousedown", onMouseDown);
      viewport?.removeEventListener("resize", lift);
      viewport?.removeEventListener("scroll", lift);
      bar.remove();
      sticky.reset(); // a modifier left locked must not outlive the bar showing it
    },
  };
}
