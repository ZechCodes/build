// The lab's chaotic variants, drawn pill by pill. A variant gives each pill
// it moves a pose for every moment (an offset from where HeroField put it,
// a turn, a scale and an opacity); this samples it into one Web Animation
// of transform and opacity, so it runs on the compositor and the clock only
// seeks it. A drift, along the pill's lane, runs on its own `translate`, a
// straight line or an endless loop (loop.js), so a pose never has to carry
// a lane's travel or its wrap.
import { seek } from "../hero/flood.js";

// Samples a second: enough for a jolt of a tenth of a second to keep its shape.
const SAMPLES_PER_SECOND = 60;

const fixed = (value, places) => Number(value.toFixed(places));

export function poseFrame({ x, y, scale, rotate, opacity }) {
  return {
    transform: `translate(${fixed(x, 1)}px, ${fixed(y, 1)}px) rotate(${fixed(rotate, 2)}deg) scale(${fixed(scale, 3)})`,
    opacity: String(fixed(opacity, 3)),
  };
}

function sample(pose, span) {
  const steps = Math.max(1, Math.ceil(span * SAMPLES_PER_SECOND));
  return Array.from({ length: steps + 1 }, (_, step) => ({ offset: step / steps, ...poseFrame(pose((span * step) / steps)) }));
}

const translate = (x) => ({ translate: `${fixed(x, 1)}px 0px` });

// Finite: the lane's speed until the pill's last moment. Endless: its loop.
function driftAnimation(element, drift, span, endless) {
  if (endless) {
    const { from, to, duration, iterationStart } = drift;
    return element.animate([translate(from), translate(to)], { duration, iterationStart, iterations: Infinity, easing: "linear" });
  }
  return element.animate([translate(0), translate(drift.speed * span)], { duration: span * 1000, fill: "both", easing: "linear" });
}

/** `pills`: { element, pose(t), until (finite), drift?, z? }. `end` is the
 *  beat's end, Infinity when endless, and then each pose loops over
 *  `period` seconds. `hidden` are the pills the variant leaves out. */
export function posedMotion({ pills, hidden = [], end, period }) {
  const endless = end === Infinity;
  // Each animation and how long it runs before it holds.
  const timed = [];
  for (const element of hidden) element.style.visibility = "hidden";
  // Created held, so none runs ahead of the clock while the rest are built.
  const hold = (animation) => {
    animation.pause();
    return animation;
  };
  for (const pill of pills) {
    const span = endless ? period : pill.until;
    const runs = endless ? Infinity : span * 1000;
    if (pill.z !== undefined) pill.element.style.zIndex = String(pill.z);
    timed.push([hold(pill.element.animate(sample(pill.pose, span), endless
      ? { duration: span * 1000, iterations: Infinity, easing: "linear" }
      : { duration: span * 1000, fill: "both", easing: "linear" })), runs]);
    if (pill.drift) timed.push([hold(driftAnimation(pill.element, pill.drift, span, endless)), runs]);
  }
  // An animation held at its end is left alone until the clock goes back:
  // seeking hundreds of finished ones every frame would restyle them all.
  const held = new Set();
  return {
    end,
    update(time, playing, rate) {
      const ms = time * 1000;
      for (const [animation, runs] of timed) {
        if (ms >= runs && (held.has(animation) || animation.playState === "finished")) {
          // Run out on its own, it already holds its end.
          held.add(animation);
          continue;
        }
        // One the compositor has not started yet holds its time; seeking it
        // again would only put its start off further.
        if (playing && animation.pending) continue;
        seek(animation, ms, playing, runs, rate);
        if (ms >= runs) held.add(animation);
        else held.delete(animation);
      }
    },
  };
}
