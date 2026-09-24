// Moving the git status above the composer, one character at a time.
//
// The line is a row of cells from core/gitStatusCells.js, and everything that
// can happen to it is one of three moves:
//
//   roll   a cell stays where it is while its old character rolls up and out of
//          the top and the new one rolls up into its place. Only those two are
//          ever drawn — 1 → 7 shows 1 and 7, never the six numbers between.
//   cascade characters arrive one beat apart from the right, travelling up and
//          left; they leave one beat apart from the left, travelling up and
//          right. Arrivals run rightmost first, departures leftmost first, so
//          the two read as opposites of each other.
//   slide  a cell whose place moved animates from where it was to where it is,
//          so a section arriving or leaving never makes its neighbours jump.
//
// The phases run in that order: the old characters leave, the room they held is
// given back (and any changed digits roll) while the survivors slide, and only
// then do the new characters cascade in. A status arriving mid-move is held —
// the line finishes what it is doing and then goes straight to the newest one,
// so the reader never watches it chase a queue.

import { setHidden } from "../dom.js";
import { MOTION_BEAT_MS, MOTION_DURATION_MS, MOTION_EASING, prefersReducedMotion } from "./motion.js";
import { gitStatusPlan } from "./gitStatusCells.js";

const CELL = "data-cell";
const CELL_CLASS = "gitcell";
const GLYPH_CLASS = "gitglyph";
const ARRIVING_CLASS = "gitglyph-arriving";

/// How far along the line a character travels as it comes or goes. Small: it
/// says which way the move is going without the character sliding in from off
/// the end of the line.
const TRAVEL = "0.6em";

const CASCADE_IN = [
  { transform: `translate(${TRAVEL}, 100%)`, opacity: 0 },
  { transform: "translate(0, 0)", opacity: 1 },
];
const CASCADE_OUT = [
  { transform: "translate(0, 0)", opacity: 1 },
  { transform: `translate(${TRAVEL}, -100%)`, opacity: 0 },
];
const ROLL_OUT = [
  { transform: "translateY(0)", opacity: 1 },
  { transform: "translateY(-100%)", opacity: 0 },
];
const ROLL_IN = [
  { transform: "translateY(100%)", opacity: 0 },
  { transform: "translateY(0)", opacity: 1 },
];

const timing = (delay = 0) => ({ duration: MOTION_DURATION_MS, easing: MOTION_EASING, delay, fill: "both" });

export function createGitStatusTicker(host) {
  const page = host.ownerDocument;

  let painted = []; // the cells standing in the host right now
  let running = null; // the move in flight, or null when the line is still
  let pending = null; // the newest status held back while one is in flight

  const cellNamed = (key) => host.querySelector(`[${CELL}="${key}"]`);
  const glyphOf = (key) => cellNamed(key).firstElementChild;
  const animates = () => typeof host.animate === "function" && !prefersReducedMotion();

  function makeCell(cell) {
    const element = page.createElement("span");
    element.className = CELL_CLASS;
    element.setAttribute(CELL, cell.key);
    element.dataset.slot = cell.slot;
    element.dataset.section = cell.section;
    element.appendChild(makeGlyph(cell.char));
    return element;
  }

  function makeGlyph(char, arriving = false) {
    const glyph = page.createElement("span");
    glyph.className = arriving ? `${GLYPH_CLASS} ${ARRIVING_CLASS}` : GLYPH_CLASS;
    glyph.textContent = char;
    return glyph;
  }

  /// Make the host hold exactly these cells, in this order. Characters are not
  /// written here — a cell that stays keeps the character it is showing until
  /// its roll replaces it.
  function applyCells(cells, { hideArrivals = false } = {}) {
    const live = new Map([...host.children].map((child) => [child.getAttribute(CELL), child]));
    cells.forEach((cell, index) => {
      const standing = live.get(cell.key);
      const element = standing || makeCell(cell);
      if (!standing && hideArrivals) element.firstElementChild.style.setProperty("opacity", "0");
      if (host.children[index] !== element) host.insertBefore(element, host.children[index] || null);
      live.delete(cell.key);
    });
    for (const gone of live.values()) host.removeChild(gone);
  }

  const leftEdges = (keys) => new Map(keys.map((key) => [key, cellNamed(key).getBoundingClientRect().left]));

  function play(element, keyframes, delay) {
    const animation = element.animate(keyframes, timing(delay));
    return Promise.resolve(animation.finished).then(() => animation);
  }

  function cascade(keys, keyframes) {
    return Promise.all(
      keys.map((key, index) =>
        play(glyphOf(key), keyframes, index * MOTION_BEAT_MS).then((animation) => {
          const glyph = cellNamed(key) && cellNamed(key).firstElementChild;
          if (glyph) glyph.style.removeProperty("opacity");
          animation.cancel();
        }),
      ),
    );
  }

  function slide(keys, before) {
    return Promise.all(
      keys
        .map((key) => ({ key, distance: before.get(key) - cellNamed(key).getBoundingClientRect().left }))
        .filter(({ distance }) => distance !== 0)
        .map(({ key, distance }) =>
          play(
            cellNamed(key),
            [{ transform: `translateX(${distance}px)` }, { transform: "translateX(0)" }],
            0,
          ).then((animation) => animation.cancel()),
        ),
    );
  }

  function roll({ key, to }) {
    const cell = cellNamed(key);
    const leaving = cell.firstElementChild;
    const arriving = makeGlyph(to, true);
    cell.appendChild(arriving);
    return Promise.all([play(leaving, ROLL_OUT, 0), play(arriving, ROLL_IN, 0)]).then((animations) => {
      cell.removeChild(leaving);
      arriving.className = GLYPH_CLASS;
      animations.forEach((animation) => animation.cancel());
    });
  }

  function snap(cells) {
    applyCells(cells);
    cells.forEach((cell) => {
      const glyph = glyphOf(cell.key);
      glyph.style.removeProperty("opacity");
      if (glyph.textContent !== cell.char) glyph.textContent = cell.char;
    });
  }

  async function perform(plan) {
    if (!animates()) {
      snap(plan.cells);
      return;
    }
    setHidden(host, false);
    if (plan.exits.length) await cascade(plan.exits, CASCADE_OUT);
    const staying = plan.cells.filter((cell) => !plan.enters.includes(cell.key)).map((cell) => cell.key);
    const before = leftEdges(staying);
    applyCells(plan.cells, { hideArrivals: true });
    await Promise.all([slide(staying, before), ...plan.rolls.map(roll)]);
    if (plan.enters.length) await cascade(plan.enters, CASCADE_IN);
  }

  function settleHost() {
    setHidden(host, painted.length === 0);
  }

  function start(cells) {
    const plan = gitStatusPlan(painted, cells);
    painted = cells;
    if (!plan.enters.length && !plan.exits.length && !plan.rolls.length) {
      settleHost();
      return Promise.resolve();
    }
    const move = perform(plan).finally(() => {
      running = null;
      settleHost();
      const next = pending;
      pending = null;
      if (next) start(next);
    });
    running = move;
    return move;
  }

  return {
    /** Say this status, moving whatever has to move to get there. */
    show(cells) {
      if (running) {
        pending = cells;
        return running;
      }
      return start(cells);
    },

    /** The move in flight, for a caller that has to wait it out. */
    settled: () => running || Promise.resolve(),

    /** Whether the line has anything to say — including a line whose last
     *  characters are still on their way out. */
    populated: () => painted.length > 0 || running !== null,
  };
}
