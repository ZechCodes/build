// Standing in for the browser's animation engine, which jsdom has none of.
//
// Every call the motion primitive makes is recorded and left running until the
// test finishes it by hand, so a test can look at what a move asked for before
// deciding when it is over.

import { MOTION_BEAT_MS, motionSettled } from "../src/core/motion.js";

/** Every animation the primitive started, each one finished by hand. */
export function recordAnimations() {
  const started = [];
  Element.prototype.animate = function animate(keyframes, options) {
    let finish;
    const run = {
      element: this,
      keyframes,
      options,
      cancelled: false,
      finished: new Promise((resolve) => {
        finish = () => resolve(run);
      }),
      cancel() {
        run.cancelled = true;
      },
      finish() {
        finish();
      },
    };
    started.push(run);
    return run;
  };
  return started;
}

export function stopRecordingAnimations() {
  delete Element.prototype.animate;
}

/** Long enough for the queue to admit the next move: the beat it waits, and a
 *  little more for the timer to actually land. */
export const motionBeat = () => new Promise((resolve) => setTimeout(resolve, MOTION_BEAT_MS + 10));

const SETTLING_ROUNDS = 24;

export async function settleMotion(started) {
  for (let round = 0; round < SETTLING_ROUNDS; round += 1) {
    started.forEach((run) => run.finish());
    let still = false;
    motionSettled().then(() => {
      still = true;
    });
    await motionBeat();
    if (still) return;
  }
  await motionSettled();
}
