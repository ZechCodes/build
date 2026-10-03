// Variant E, the wall (#316): the field as a wall of notifications sliding
// in, showing and blurring out everywhere at once (legacy/wall.js and
// legacy/wall-motion.js preserve this experiment for the lab). At the
// end, a wave from the laptop's screen blurs the wall out for good while
// the three requests pop up fresh where it was, turn green and fly to their
// rows. Endless, the wall only cycles.
//
// The lab has no headline or laptop, so it outlines where they stand on
// the home page: the dome must stay above them, and the requests land in
// the laptop's.
import { ATTENTION } from "./legacy/field.js";
import { rippleHit, rippleReach } from "./legacy/timing.js";
import { planSlot, requestSlots, wallShape, wallSlots } from "./legacy/wall.js";
import { createWallMotion } from "./legacy/wall-motion.js";
import { clamp01, requestPose } from "./motion-parts.js";
import { posedMotion } from "./posed.js";
import { NARROW_QUERY } from "./stage.js";

// How long before its take-off a request pops up, and how far into that
// it turns green, in seconds.
const POP_LEAD = 0.45;
const GREEN = Object.freeze({ after: 0.12, over: 0.15 });
// A request's slot has blurred its last notification out by its pop.
const CLEARED_BEFORE_POP = 0.3;

const narrowNow = () => globalThis.matchMedia?.(NARROW_QUERY).matches ?? false;

function place(element, [left, top], size) {
  element.style.setProperty("left", `${left}px`);
  element.style.setProperty("top", `${top}px`);
  if (!size) return element;
  element.style.setProperty("width", `${size[0]}px`);
  element.style.setProperty("height", `${size[1]}px`);
  return element;
}

function make(document, tag, className, text = "") {
  const element = document.createElement(tag);
  element.className = className;
  element.textContent = text;
  return element;
}

function wallElement(document, shape) {
  const wall = make(document, "div", "hero-wall");
  wall.dataset.shape = shape.kind;
  if (shape.kind === "dome") {
    for (const [name, value] of [["--rx", `${shape.rx}px`], ["--ry", `${shape.ry}px`], ["--inner", String(shape.inner)], ["--fade", String(shape.fade)]]) wall.style.setProperty(name, value);
  }
  return wall;
}

// A request: its notification, and a green one over it that fades in.
function requestElement(document, entry, slot) {
  const request = place(make(document, "span", "hero-wall__request"), [slot.x, slot.y]);
  for (const className of ["hero-pill", "hero-pill hero-wall__green"]) {
    const pill = make(document, "span", className, entry.text);
    pill.dataset.harness = entry.harness;
    request.append(pill);
  }
  return request;
}

function standIns(document, stage) {
  return [["headline", stage.headline], ["laptop", stage.screen]].map(([label, [left, top, right, bottom]]) => {
    const box = place(make(document, "div", "lab-standin", label), [left, top], [right - left, bottom - top]);
    box.dataset.standin = label;
    return box;
  });
}

// The requests' motion: each pops where its slot was, turns green, flies.
function requestMotion(requests, stage) {
  const { timing } = stage;
  const posed = requests.flatMap(({ element, slot, index }) => {
    const appear = timing.landings[index] - timing.flight - POP_LEAD;
    const pill = { element, x: slot.x, y: slot.y };
    const green = element.children[1];
    const turnsGreen = (t) => ({ x: 0, y: 0, rotate: 0, scale: 1, opacity: clamp01((t - appear - GREEN.after) / GREEN.over) });
    return [
      { element, ...requestPose({ pill, index }, { stage, timing }, { appear }) },
      { element: green, pose: turnsGreen, until: timing.landings[index] },
    ];
  });
  return posedMotion({ pills: posed, end: timing.settle[1] });
}

/** The wall on the lab's field (`root`), as the other variants are built:
 *  answers update(time, playing, rate), its end, and dispose(). */
export function wallFlood({ field, stage, root, options }, { endless = false } = {}) {
  const document = root.ownerDocument;
  const { width, height } = field;
  const narrow = narrowNow();
  const shape = wallShape({ width, narrow, clear: stage.clear, kind: options.shape });
  const slots = wallSlots({ width, height, narrow, shape });
  const plans = slots.map((slot, index) => planSlot(index));
  const reach = rippleReach(stage.origin, field);
  const chosen = endless ? [] : requestSlots(slots, shape, { width, clear: stage.clear });
  const exits = slots.map((slot) => {
    const index = chosen.indexOf(slot);
    const wave = rippleHit([slot.x, slot.y], stage.origin, reach, stage.timing);
    if (index === -1) return wave;
    return Math.min(wave, stage.timing.landings[index] - stage.timing.flight - POP_LEAD - CLEARED_BEFORE_POP);
  });
  const wall = wallElement(document, shape);
  const added = [wall, ...standIns(document, stage)];
  const end = endless ? Infinity : stage.timing.settle[1];
  const motions = [createWallMotion({ root: wall, slots, plans, exits, blur: options.blur, end })];
  if (!endless) {
    const requests = ATTENTION.map((entry, index) => ({ element: requestElement(document, entry, chosen[index]), slot: chosen[index], index }));
    added.push(...requests.map(({ element }) => element));
    motions.push(requestMotion(requests, stage));
  }
  root.append(...added);
  return {
    end,
    update(time, playing, rate) {
      for (const motion of motions) motion.update(time, playing, rate);
    },
    dispose() {
      for (const element of added) element.remove();
    },
  };
}
