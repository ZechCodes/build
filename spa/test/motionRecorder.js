import { MOTION_BEAT_MS, motionSettled } from "../src/core/motion.js";
import { vi } from "vitest";

export function recordAnimations() {
  const started = [];
  running = [];
  Element.prototype.animate = function animate(keyframes, options) {
    let finish;
    const run = {
      element: this,
      keyframes,
      options,
      cancelled: false,
      over: false,
      finished: new Promise((resolve) => {
        finish = () => resolve(run);
      }),
      cancel() {
        run.cancelled = true;
      },
      finish() {
        run.over = true;
        finish();
      },
    };
    started.push(run);
    running.push(run);
    return run;
  };
  return started;
}

export function stopRecordingAnimations() {
  running = [];
  delete Element.prototype.animate;
}

export async function motionBeat() {
  const beat = new Promise((resolve) => setTimeout(resolve, MOTION_BEAT_MS + 10));
  // A suite may fake only intervals. Await the actual timeout even when
  // advancing the fake clock, so that partial clocks still yield a real beat.
  if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(MOTION_BEAT_MS + 10);
  await beat;
}

const SETTLING_ROUNDS = 24;

let running = [];

export async function settleMotion() {
  for (let round = 0; round < SETTLING_ROUNDS; round += 1) {
    running.forEach((run) => run.finish());
    let still = false;
    motionSettled().then(() => {
      still = true;
    });
    await motionBeat();
    if (still && running.every((run) => run.over || run.cancelled)) return;
  }
  await motionSettled();
}
