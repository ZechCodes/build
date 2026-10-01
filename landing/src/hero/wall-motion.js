// The notification wall (#316) on the page: wall.js's slots drawn as pills
// over the field and their keyframes run as Web Animations of transform
// and opacity (and, with the "filter" blur, filter), so the compositor
// plays them and a clock only seeks them (seeker.js).
//
// Each slot is two pills, one per notification, and with the "copy" blur a
// blurred copy of each (wall.css draws its blur with shadows, painted
// once), which takes over as the sharp one fades.
import { noteFrames, slotCycles } from "./wall.js";
import { seekAll } from "./seeker.js";

const create = (document, tag, className) => {
  const element = document.createElement(tag);
  element.className = className;
  return element;
};

function pillOf(document, slot, note, className) {
  const pill = create(document, "span", className);
  pill.dataset.harness = note.harness;
  pill.textContent = note.text;
  pill.style.setProperty("left", `${slot.x}px`);
  pill.style.setProperty("top", `${slot.y}px`);
  pill.style.setProperty("--size", String(slot.size));
  return pill;
}

// Where in the slot's period a note's own clock starts, as a share of it:
// endless, each note loops its one turn from there.
const iterationStartOf = (plan, note) => {
  const share = -(plan.phase + plan.notes[note].start) / plan.period;
  return share - Math.floor(share);
};

function timingOf(plan, note, end) {
  if (end === Infinity) return { duration: plan.period * 1000, iterations: Infinity, iterationStart: iterationStartOf(plan, note), easing: "linear" };
  return { duration: end * 1000, fill: "both", easing: "linear" };
}

/** Draws the wall into `root` (its own element, already in the field) and
 *  answers update(time, playing, rate). `exits[i]` is when slot i blurs
 *  out for good; `end` the beat's end, Infinity when endless. */
export function createWallMotion({ root, slots, plans, exits, blur, end }) {
  const document = root.ownerDocument;
  const timed = [];
  const runs = end === Infinity ? Infinity : end * 1000;
  // Created held, so none runs ahead of the clock while the rest are built.
  const animate = (element, frames, timing) => {
    const animation = element.animate(frames, timing);
    animation.pause();
    timed.push([animation, runs]);
  };
  slots.forEach((slot, index) => {
    const plan = plans[index];
    slot.notes.forEach((note, which) => {
      const cycles = slotCycles(plan, which, { exit: end === Infinity ? Infinity : exits[index] });
      const { sharp, blurred } = noteFrames(cycles, { span: end === Infinity ? plan.period : end, peak: slot.peak, blur });
      const timing = timingOf(plan, which, end);
      const pill = pillOf(document, slot, note, "hero-pill hero-wall__pill");
      root.append(pill);
      animate(pill, sharp, timing);
      if (!blurred) return;
      const copy = pillOf(document, slot, note, "hero-pill hero-wall__pill hero-wall__blur");
      root.append(copy);
      animate(copy, blurred, timing);
    });
  });
  return { end, update: seekAll(timed) };
}
