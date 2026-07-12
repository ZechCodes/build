# Stage 01 — Touch scrolling for terminal panes

## Goal

Terminal scrolling on mobile doesn't work: touch-dragging on any terminal pane
(user terminals, the agent pane, the main-worktree terminal) does nothing. Fix it
so a one-finger drag scrolls the terminal, a flick scrolls with momentum, and a
tap still focuses the terminal (bringing up the keyboard), on both the normal
screen (scrollback) and the alternate screen (full-screen TUIs like the Claude
Code harness).

All work is in `spa/`. No bridge/server changes.

## Root cause (verified)

The web client renders terminals with **ghostty-web 0.4.0** (canvas-based), not
xterm.js. Its `Terminal.open(parent)` registers **only mouse and wheel
listeners** on the element you pass in — `mousedown`, `mousemove`, `mouseleave`,
`click`, `mouseup`, and `wheel` — and **zero touch or pointer handlers** (see
`node_modules/ghostty-web/dist/ghostty-web.js`, the `open(A)` method and the
listener registration around the constructor). On a touch device, a drag on the
canvas emits only touch events, so nothing scrolls.

Two facts about ghostty-web make the fix small:

1. **`open(parent)` sets `this.element = parent`** — the wheel listener lives on
   the exact host element our code passes to `term.open(host)` in
   `spa/src/terminal/pane.js:25`. A synthetic `WheelEvent` dispatched at that
   host triggers ghostty's own `handleWheel`.
2. **`handleWheel` already does the right thing in both screen modes**:
   - alternate screen → emits arrow-key escapes (`\x1B[A` / `\x1B[B`) to the PTY,
     quantized at one arrow per 33px of `deltaY`, capped at 5 per event;
   - normal screen → converts `DOM_DELTA_PIXEL` deltas to lines using the real
     cell height and calls `smoothScrollTo`.

So the fix is: **translate touch gestures into synthetic `WheelEvent`s dispatched
at the pane host**, reusing all of ghostty's scroll logic instead of
reimplementing mode detection, line conversion, or smooth scrolling. (The public
API also offers `scrollLines()` etc., but driving those directly would skip the
alternate-screen arrow-key path — don't.)

## Why not an existing package? (build-vs-buy, surveyed 2026-07-11)

Reviewer question: has no one built this already? Surveyed the realistic
options; none fits. The custom module is ~150 lines of dependency-free,
unit-tested code, and the parts a package could supply are the easy parts.

**Upstream ghostty-web — doesn't have it, even unreleased.** Checked npm: the
latest stable is 0.4.0 (what we pin, Dec 2025), and the newest prerelease
`0.4.0-next.20.g1858a59` (2026-06-28, from `coder/ghostty-web`) adds exactly one
touch listener — `touchend → preventDefault() + focus()` on the canvas, for
mobile keyboard focus. No `touchstart`/`touchmove`, no scrolling. So there is no
version to upgrade to, and no upstream implementation to crib. (Worth filing an
upstream issue offering our gesture module once it's proven here — if it lands
upstream later, `touchScroll.js` is a self-contained module we can delete.)

**xterm.js — has touch scrolling, but it's a renderer swap.** The one terminal
package that handles touch natively. Migrating the renderer to get scrolling is
wildly out of proportion to the bug and out of scope for this stage.

**Generic gesture/momentum libraries — dead, mismatched, or both:**

- `hammerjs` 2.0.8 — last release **April 2016**, unmaintained. Recognizes
  pan/swipe but has no momentum loop and nothing terminal-aware.
- `impetus` 0.8.8 (Nov 2018) and `zingtouch` 1.0.6 — likewise unmaintained for
  years. Impetus is the closest shape (touch drag → decaying value callback)
  but is abandoned and still leaves the terminal-specific work to us.
- `better-scroll` 2.5.1 (Mar 2023) / iScroll — scroll a DOM **content element**
  via CSS transforms. A ghostty terminal is a canvas with no scrollable content
  element; scrollback lives inside the wasm terminal. Wrong model entirely.
- `interactjs` — maintained, but built for drag/drop/resize of positioned
  elements; its inertia moves elements, it doesn't emit scroll deltas.
- `@use-gesture/vanilla` 10.3.1 (Mar 2024) — the only live, plausible option.
  It would supply drag tracking with velocity, but delivers callbacks: we would
  still write the tap-slop/keyboard-focus preservation, the synthetic-wheel
  dispatch, and the momentum decay loop — i.e. most of `touchScroll.js` — while
  adding a React-ecosystem dependency to an E2EE client whose security
  checklist we keep at 100/100.

**The real reason no package fits:** the novel work here isn't gesture
recognition (tracking one finger's `clientY` is trivial); it's the domain glue —
(a) preserving tap-to-focus so the mobile keyboard still appears, (b) routing
scrolls through ghostty's own `handleWheel` so the alternate screen gets
arrow-key escapes instead of viewport scrolling, and (c) synthesizing
pixel-mode `WheelEvent`s at the host ghostty listens on. No library on npm does
any of that, because it's specific to ghostty-web's event model.

## Where terminals mount

`mountTerminalPane(host, …)` in `spa/src/terminal/pane.js` is the single
choke-point — every terminal in the app goes through it:

- `spa/src/core/surfaceTabs.js` — `mountUserTerminalPane` / `mountAgentPane` /
  `mountAuxTab` (hosts are `.termpane` divs),
- `spa/src/views/task.js` — agent pane,
- `spa/src/views/mainWorktree.js`, `spa/src/views/worktree.js` — via the above.

Touching only `pane.js` (plus one new module and CSS) fixes every surface.

## Design

### New pure module: `spa/src/terminal/touchScroll.js`

A DOM-free gesture state machine so it is unit-testable in vitest's default
**node environment** (this repo configures no jsdom/happy-dom — see
`spa/vite.config.js`; existing tests like `spa/test/terminal.test.js` are pure
JS with fakes). All effects are injected:

```js
/**
 * createTouchScroll({ dispatchWheel, requestFrame, cancelFrame }) →
 *   { onTouchStart, onTouchMove, onTouchEnd, onTouchCancel, dispose }
 *
 * dispatchWheel(deltaYPx) — effect: deliver a pixel-mode wheel delta.
 * requestFrame(cb)        — rAF injection; cb receives a timestamp (ms).
 * cancelFrame(id)         — cancel a pending frame.
 */
```

Handlers take real TouchEvent-shaped objects but read only
`touches`/`changedTouches` (`identifier`, `clientY`), `timeStamp`, and call
`preventDefault()` — trivially fakeable as plain objects in tests.

Gesture rules (all TDD'd — tests first):

- **Sign**: `deltaY = previousClientY - currentClientY`. Finger moving up ⇒
  positive `deltaY` ⇒ scroll down (matches native touch scrolling; matches
  ghostty's alt-screen mapping of `deltaY > 0` → down-arrow).
- **Tap preservation / slop**: never `preventDefault()` on `touchstart`; emit
  nothing and don't `preventDefault()` until cumulative movement exceeds an
  8px slop, so browser-synthesized `click` still fires and ghostty's
  click/focus handling brings up the mobile keyboard. Once past slop,
  `preventDefault()` every `touchmove` and emit deltas (including the banked
  slop distance on the first emission).
- **Single-finger only**: track one touch by `identifier`; a second concurrent
  touch ends the gesture (no momentum) and further moves are ignored until all
  fingers lift.
- **Momentum**: sample velocity over the last ~100ms of moves (use
  `event.timeStamp`, never `Date.now()`). On `touchend`, if speed exceeds
  ~0.05 px/ms, start a frame loop via `requestFrame`: each frame dispatches
  `velocity * dt` and decays velocity exponentially (halve roughly every
  300ms, i.e. `v *= Math.pow(0.5, dt / 300)`); stop below ~0.01 px/ms. Frame
  timestamps come from the injected `requestFrame` callback argument.
- **Interruption**: a new `touchstart`, `touchcancel`, or `dispose()` cancels
  any running momentum loop (via `cancelFrame`) and resets state.

Exact thresholds are implementation freedom; the behaviors (slop, sign,
single-finger, decay-to-stop, interruption) are the contract the tests pin.

### Wiring in `spa/src/terminal/pane.js`

In `mountTerminalPane` after `term.open(host)`:

- `host.style.touchAction = "none"` — stops the browser from panning/zooming
  the page from gestures that start on the terminal (setting it here covers
  every mount site; no per-callsite CSS to forget).
- Create the gesture with real effects:
  - `dispatchWheel: (deltaY) => host.dispatchEvent(new WheelEvent("wheel", { deltaY, deltaMode: WheelEvent.DOM_DELTA_PIXEL, bubbles: true, cancelable: true }))`
    — ghostty's listener is on `host` itself (capture), so at-target dispatch
    reaches it; its handler `preventDefault()`s and stops propagation.
  - `requestFrame` / `cancelFrame`: `requestAnimationFrame` /
    `cancelAnimationFrame` bound to `window`.
- `host.addEventListener("touchstart", g.onTouchStart, { passive: true })`,
  `("touchmove", g.onTouchMove, { passive: false })` (must be non-passive —
  the handler calls `preventDefault()`), `("touchend", g.onTouchEnd)`,
  `("touchcancel", g.onTouchCancel)`.
- In the returned `dispose()`: remove the four listeners and call
  `g.dispose()` (kills any momentum frame), alongside the existing cleanup.

No changes to the attach/input/resize plumbing, `spa/src/terminal/session.js`,
or `spa/src/terminal/manager.js`.

### CSS

`spa/src/styles.css`: no structural changes required (the `touch-action` is set
inline by `pane.js`). Do **not** add `touch-action` rules to `.termpane`/`.tpane`
in CSS — keeping it in `mountTerminalPane` guarantees it travels with every
future mount site.

## TDD plan (write these first, watch them fail)

New file `spa/test/touchScroll.test.js`, node environment, plain-object fake
events — follow the style of `spa/test/terminal.test.js` (fakes + collected
effect logs). Helpers:

```js
const touch = (id, y) => ({ identifier: id, clientY: y, clientX: 0 });
const ev = (touches, t, changed = touches) => ({
  touches, changedTouches: changed, timeStamp: t,
  prevented: false, preventDefault() { this.prevented = true; },
});
```

and a fake frame scheduler (array of pending callbacks; test fires them with
chosen timestamps). Cases:

1. **Drag scrolls with the right sign** — start at y=300, move to y=200 across
   several moves ⇒ sum of dispatched deltas ≈ +100 (finger up ⇒ scroll down);
   the reverse drag dispatches negative deltas.
2. **Tap stays a tap** — start + move 4px + end ⇒ zero dispatches and no
   `preventDefault()` on any event.
3. **Slop then scroll** — moves of 5px then 10px ⇒ first move emits nothing and
   is not prevented; second is prevented and the emitted total ≈ 15px.
4. **Second finger ends the gesture** — mid-drag, a two-touch event stops
   emission; later single-touch moves in the same gesture emit nothing.
5. **Flick produces momentum** — fast drag then `touchend` ⇒ frames requested;
   firing fake frames with advancing timestamps dispatches decaying same-sign
   deltas and eventually stops requesting frames.
6. **Slow release ⇒ no momentum** — drag with a ≥100ms stationary pause before
   `touchend` ⇒ no frame requested.
7. **New touch interrupts momentum** — `touchstart` during momentum ⇒
   `cancelFrame` called, no further dispatches from the old loop.
8. **`dispose()` cancels a pending frame.**

Then implement `touchScroll.js` until green, then wire `pane.js` (the wiring is
thin, DOM-bound glue — covered by verification below, not unit tests).

## Setup note for a cold agent

`spa/node_modules` is not installed in this worktree, and the
`@build/secure-transport` dependency is a file dep at `file:../../build-secure-transport/js`
(resolved relative to `spa/`, i.e. two levels above the repo root). Before
`npm install`, make it reachable from the worktree parent:

```sh
ln -sfn ~/Projects/8ly/build-secure-transport \
  "$(git rev-parse --show-toplevel)/../build-secure-transport"
cd spa && npm install
```

(`~/Projects/8ly/build-secure-transport/js` exists on this machine.)

## Verification

- `cd spa && npm test` — new suite green, all existing suites stay green.
- Manual, end-to-end: `cd spa && npm run dev`, open in a Chromium browser,
  toggle DevTools device emulation (touch), open any worktree's terminal tab
  (or a task's agent pane), produce enough output to scroll (e.g. `seq 1 200`),
  then:
  - drag up ⇒ scrollback moves; flick ⇒ momentum; tap ⇒ keyboard focus intact;
  - `window.__buildTerminal.getViewportY()` in the console reports > 0 after an
    upward drag and returns to 0 after `scrollToBottom` / dragging back down;
  - run a full-screen TUI (e.g. `less` on a long file) ⇒ drags emit arrow keys
    (content moves) instead of viewport scrolling;
  - confirm a gesture starting on the terminal no longer pans the page.
- `cargo` checks don't apply (no `bridge/` changes).

## Commit

Run `semgrep` and `gitleaks` before committing (repo rule). Suggested message:
`fix(spa): terminal panes scroll on touch — gesture→wheel synthesis with momentum`.
Commit only `spa/src/terminal/touchScroll.js`, `spa/src/terminal/pane.js`, and
`spa/test/touchScroll.test.js`; write nothing outside the repo and do not commit
the sibling symlink (it lives outside the worktree).
