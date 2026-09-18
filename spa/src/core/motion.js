export const MOTION_DURATION_MS = 180;
export const MOTION_BEAT_MS = 40;
export const MOTION_EASING = "cubic-bezier(0.2, 0, 0, 1)";

const REVEAL = "reveal";
const HIDE = "hide";
const OPACITY = "opacity";

const BORROWED_PROPERTIES = ["width", "height", "opacity", "overflow"];

const moves = new WeakMap();
const rowContentRevisions = new WeakMap();
const rowResizeAnimations = new WeakMap();

let queue = Promise.resolve();
let moving = 0;
const waitingForStillness = [];

export function prefersReducedMotion() {
  const match = globalThis.matchMedia;
  if (typeof match !== "function") return false;
  const query = match("(prefers-reduced-motion: reduce)");
  return !!(query && query.matches);
}

export function motionSettled() {
  if (moving === 0) return Promise.resolve();
  return new Promise((resolve) => waitingForStillness.push(resolve));
}

export function reveal(element, { axis = "width" } = {}) {
  return move(element, REVEAL, axis);
}

export function hide(element, { axis = "width" } = {}) {
  return move(element, HIDE, axis);
}

export function settleHidden(element) {
  const standing = moves.get(element);
  if (standing) countermand(element, standing);
  settle(element, HIDE);
}

export function motionHooks({ axis = "width" } = {}) {
  return {
    onEnter: (element) => {
      element.hidden = true;
      return reveal(element, { axis });
    },
    onExit: (element) => hide(element, { axis }),
  };
}

/** Replace a whole optional row without throwing its exiting contents away
 * before the height animation can measure and show them. A new paint
 * counteracts a pending exit and owns the eventual cleanup. */
export function setMotionRowHtml(element, html) {
  const revision = (rowContentRevisions.get(element) || 0) + 1;
  rowContentRevisions.set(element, revision);
  if (html) {
    rowResizeAnimations.get(element)?.cancel();
    rowResizeAnimations.delete(element);
    const previousHtml = element.innerHTML;
    const previousHeight = naturalSize(element, "height");
    element.innerHTML = html;
    if (previousHtml && previousHtml !== html) animateRowResize(element, previousHeight, previousHtml);
    return reveal(element, { axis: "height" });
  }
  return hide(element, { axis: "height" }).then(() => {
    if (rowContentRevisions.get(element) === revision) element.innerHTML = "";
  });
}

function animateRowResize(element, previousHeight, previousHtml) {
  if (!element.isConnected || prefersReducedMotion() || typeof element.animate !== "function") return;
  const nextHeight = naturalSize(element, "height");
  const snapshot = document.createElement("div");
  snapshot.setAttribute("data-motion-snapshot", "");
  snapshot.setAttribute("aria-hidden", "true");
  snapshot.inert = true;
  snapshot.innerHTML = previousHtml;
  snapshot.querySelectorAll("[id]").forEach((child) => child.removeAttribute("id"));
  Object.assign(snapshot.style, {
    position: "absolute", inset: "0", pointerEvents: "none", display: "flex",
    flexWrap: "wrap", gap: "inherit", padding: "inherit", alignItems: "inherit",
  });
  element.style.position = "relative";
  element.appendChild(snapshot);
  const resize = element.animate(
    [{ height: `${previousHeight}px` }, { height: `${nextHeight}px` }],
    { duration: MOTION_DURATION_MS, easing: MOTION_EASING },
  );
  const fade = snapshot.animate([{ opacity: 1 }, { opacity: 0 }], {
    duration: MOTION_DURATION_MS, easing: MOTION_EASING,
  });
  const arrivals = [...element.children]
    .filter((child) => child !== snapshot)
    .map((child) => child.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: MOTION_DURATION_MS, easing: MOTION_EASING,
    }));
  const run = {
    cancel() {
      resize.cancel();
      fade.cancel();
      arrivals.forEach((animation) => animation.cancel());
      snapshot.remove();
    },
  };
  rowResizeAnimations.set(element, run);
  Promise.all([resize, fade, ...arrivals].map((animation) => Promise.resolve(animation.finished).catch(() => {}))).then(() => {
    snapshot.remove();
    if (rowResizeAnimations.get(element) === run) rowResizeAnimations.delete(element);
  });
}

function move(element, direction, axis) {
  const standing = moves.get(element);
  if (standing && standing.direction === direction) return standing.finished;
  if (standing) countermand(element, standing);
  else if (isShown(element) === (direction === REVEAL)) return Promise.resolve();

  if (!element.isConnected || prefersReducedMotion() || typeof element.animate !== "function") {
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
  enqueue(() => play(element, run).then(run.done, (error) => failMove(element, run, error)));
  return run.finished;
}

function failMove(element, run, error) {
  forget(element, run);
  settle(element, run.direction);
  run.fail(error);
}

function forget(element, run) {
  if (moves.get(element) === run) moves.delete(element);
}

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

function naturalSize(element, axis) {
  const box = element.getBoundingClientRect();
  const measured = axis === "height" ? box.height : box.width;
  if (measured > 0) return measured;
  return axis === "height" ? element.scrollHeight : element.scrollWidth;
}

function endsOfMove(element, axis) {
  if (axis === OPACITY) return { grown: { opacity: 1 }, gone: { opacity: 0 } };
  return {
    grown: { [axis]: `${naturalSize(element, axis)}px`, opacity: 1 },
    gone: { [axis]: "0px", opacity: 0 },
  };
}

async function play(element, run) {
  if (run.countermanded) return;
  const { axis, direction } = run;

  element.hidden = false;
  giveBackBorrowedProperties(element);
  const { grown, gone } = endsOfMove(element, axis);
  const from = direction === REVEAL ? gone : grown;
  const to = direction === REVEAL ? grown : gone;

  if (axis !== OPACITY) element.style.setProperty("overflow", "hidden");
  for (const [property, value] of Object.entries(from)) element.style.setProperty(property, String(value));
  run.animation = element.animate([from, to], {
    duration: MOTION_DURATION_MS,
    easing: MOTION_EASING,
    fill: "both",
  });

  const ran = Promise.resolve(run.animation.finished).catch(() => {});
  await Promise.race([ran, run.stopped]);
  if (run.countermanded) return;
  settle(element, direction);
  run.animation.cancel();
  forget(element, run);
}

function countermand(element, run) {
  run.countermanded = true;
  forget(element, run);
  if (run.animation) run.animation.cancel();
  run.stop();
  run.done();
}

function enqueue(start) {
  moving += 1;
  const admitted = queue;
  queue = admitted.then(() => pause(MOTION_BEAT_MS));
  admitted.then(start).finally(release);
}

function release() {
  moving -= 1;
  if (moving > 0) return;
  for (const resolve of waitingForStillness.splice(0)) resolve();
}

function pause(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
