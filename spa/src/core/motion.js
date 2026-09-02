// The one way anything enters or leaves a surface.
//
// A row that appears by simply being in the document is a jump: the reader's
// eye is pulled to a place that was something else a frame ago, and a row that
// disappears takes its neighbours' positions with it before the eye can follow.
// So arriving and leaving are one pair of calls — `reveal` and `hide` — and
// every surface goes through them rather than writing its own transition.
//
// ## What a move is
//
// A move animates one axis and opacity together: a pill grows along its width
// from nothing to the size layout would have given it, a viewer or a menu grows
// along its height. The inline sizes the animation needs exist only while it
// runs; when it is over they are cleared and the element is back to whatever
// the stylesheet says it is. An exit ends with `hidden` set, so the caller can
// ask "is it shown?" of the element itself.
//
// ## Why there is a queue
//
// Three pills arriving in one paint all animating at once reads as the row
// flickering rather than as three things arriving. So every call goes through
// one module-level queue and each move starts a beat after the one before it.
// The queue holds starts, not finishes: the beat is measured from the moment a
// move begins, so a slow move never stalls the ones behind it.
//
// ## What "less movement" means here
//
// `prefers-reduced-motion: reduce` makes every call an immediate state change
// through the same two functions. Callers never ask about the preference and
// never branch on it — there is one code path, and this module decides what it
// does.

export const MOTION_DURATION_MS = 180;
export const MOTION_BEAT_MS = 40;
export const MOTION_EASING = "cubic-bezier(0.2, 0, 0, 1)";

const REVEAL = "reveal";
const HIDE = "hide";

/** The properties a move borrows for its length and gives back afterwards. */
const BORROWED_PROPERTIES = ["width", "height", "opacity", "overflow"];

const moves = new WeakMap();

let queue = Promise.resolve();
let moving = 0;
const waitingForStillness = [];

/** Whether the reader has asked for less movement. matchMedia is missing in
 *  jsdom and in older embeddings, and an absent query is not a preference. */
export function prefersReducedMotion() {
  const match = globalThis.matchMedia;
  if (typeof match !== "function") return false;
  const query = match("(prefers-reduced-motion: reduce)");
  return !!(query && query.matches);
}

/** Resolves once nothing is moving — the queue is empty and every move it held
 *  has finished or been countermanded. */
export function motionSettled() {
  if (moving === 0) return Promise.resolve();
  return new Promise((resolve) => waitingForStillness.push(resolve));
}

/// Bring `element` into the layout, growing it from nothing along `axis`.
///
/// Resolves when it is standing at its natural size with the inline sizes
/// cleared. An element that is already shown resolves at once.
export function reveal(element, { axis = "width" } = {}) {
  return move(element, REVEAL, axis);
}

/// Take `element` out of the layout, shrinking it to nothing along `axis`.
///
/// Resolves once it is `hidden` — never before, so a caller may remove it from
/// the document on the promise. An element that is already hidden resolves at
/// once.
export function hide(element, { axis = "width" } = {}) {
  return move(element, HIDE, axis);
}

function move(element, direction, axis) {
  const standing = moves.get(element);
  if (standing && standing.direction === direction) return standing.finished;
  if (standing) countermand(standing);
  else if (isShown(element) === (direction === REVEAL)) return Promise.resolve();

  if (prefersReducedMotion() || typeof element.animate !== "function") {
    settle(element, direction);
    return Promise.resolve();
  }

  const run = { direction, axis, animation: null, countermanded: false };
  run.finished = new Promise((resolve, reject) => {
    run.done = resolve;
    run.fail = reject;
  });
  run.stopped = new Promise((resolve) => {
    run.stop = resolve;
  });
  moves.set(element, run);
  enqueue(() => play(element, run).then(run.done, run.fail));
  return run.finished;
}

/// What the element is on its way to being, which is what a second call about
/// the same element is answered against: an element half-way through its exit
/// is already hidden as far as another `hide` is concerned.
function isShown(element) {
  const standing = moves.get(element);
  if (standing) return standing.direction === REVEAL;
  return element.hidden !== true;
}

function settle(element, direction) {
  element.hidden = direction === HIDE;
  giveBackBorrowedProperties(element);
}

function giveBackBorrowedProperties(element) {
  for (const property of BORROWED_PROPERTIES) element.style.removeProperty(property);
}

/// The size layout gives the element when nothing is holding it.
///
/// `getBoundingClientRect` is the honest answer for an element the browser has
/// laid out; `scrollWidth`/`scrollHeight` answers for one whose box the layout
/// has not measured yet, which is where a freshly inserted row starts.
function naturalSize(element, axis) {
  const box = element.getBoundingClientRect();
  const measured = axis === "height" ? box.height : box.width;
  if (measured > 0) return measured;
  return axis === "height" ? element.scrollHeight : element.scrollWidth;
}

async function play(element, run) {
  if (run.countermanded) return;
  const { axis, direction } = run;

  element.hidden = false;
  giveBackBorrowedProperties(element);
  const grown = { [axis]: `${naturalSize(element, axis)}px`, opacity: 1 };
  const gone = { [axis]: "0px", opacity: 0 };
  const from = direction === REVEAL ? gone : grown;
  const to = direction === REVEAL ? grown : gone;

  element.style.setProperty("overflow", "hidden");
  element.style.setProperty(axis, from[axis]);
  element.style.setProperty("opacity", String(from.opacity));
  run.animation = element.animate([from, to], {
    duration: MOTION_DURATION_MS,
    easing: MOTION_EASING,
    fill: "both",
  });

  const ran = Promise.resolve(run.animation.finished).catch(() => {});
  await Promise.race([ran, run.stopped]);
  if (run.countermanded) return;
  // The styles go back to the stylesheet before the animation lets go of them,
  // so the element is never drawn at the size the keyframes ended on.
  settle(element, direction);
  run.animation.cancel();
  if (moves.get(element) === run) moves.delete(element);
}

/// Stop a move the other direction has overtaken.
///
/// The running animation is cancelled rather than left to fight the new one,
/// and the caller waiting on the countermanded move is answered — it is over,
/// even though it did not arrive.
function countermand(run) {
  run.countermanded = true;
  if (run.animation) run.animation.cancel();
  run.stop();
  run.done();
}

/// Admit one move a beat after the one before it started.
///
/// The chain advances on the start and the beat alone, so a move whose
/// animation never finishes cannot wedge the queue behind it.
function enqueue(start) {
  moving += 1;
  const admitted = queue;
  queue = admitted.then(() => pause(MOTION_BEAT_MS));
  admitted.then(start).then(release);
}

function release() {
  moving -= 1;
  if (moving > 0) return;
  for (const resolve of waitingForStillness.splice(0)) resolve();
}

function pause(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
