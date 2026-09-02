// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { motionBeat as tick, recordAnimations, stopRecordingAnimations } from "./motionRecorder.js";
import {
  MOTION_BEAT_MS,
  MOTION_DURATION_MS,
  MOTION_EASING,
  hide,
  motionHooks,
  motionSettled,
  reveal,
  settleHidden,
} from "../src/core/motion.js";

const boxOf = (width, height) => ({ width, height, top: 0, left: 0, right: width, bottom: height });

function elementSized(width = 120, height = 24) {
  const element = document.createElement("span");
  element.getBoundingClientRect = () => boxOf(width, height);
  document.body.appendChild(element);
  return element;
}

const elementHidden = (width, height) => {
  const element = elementSized(width, height);
  element.hidden = true;
  return element;
};

const askForLessMotion = () => {
  globalThis.matchMedia = () => ({ matches: true });
};

const inlineStyleOf = (element) => element.getAttribute("style") || "";

afterEach(async () => {
  await motionSettled();
  stopRecordingAnimations();
  delete globalThis.matchMedia;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("revealing an element", () => {
  it("grows it from nothing to its natural size, then hands the size back to layout", async () => {
    const started = recordAnimations();
    const pill = elementHidden(120);

    const revealed = reveal(pill, { axis: "width" });
    await tick();

    expect(pill.hidden).toBe(false);
    expect(started).toHaveLength(1);
    expect(started[0].element).toBe(pill);
    expect(started[0].keyframes).toEqual([
      { width: "0px", opacity: 0 },
      { width: "120px", opacity: 1 },
    ]);
    expect(started[0].options).toEqual({
      duration: MOTION_DURATION_MS,
      easing: MOTION_EASING,
      fill: "both",
    });
    expect(pill.style.overflow).toBe("hidden");

    started[0].finish();
    await revealed;

    expect(pill.hidden).toBe(false);
    expect(inlineStyleOf(pill)).toBe("");
  });

  it("grows a taller element by its height when that is the axis asked for", async () => {
    const started = recordAnimations();
    const viewer = elementHidden(400, 260);

    const revealed = reveal(viewer, { axis: "height" });
    await tick();
    expect(started[0].keyframes).toEqual([
      { height: "0px", opacity: 0 },
      { height: "260px", opacity: 1 },
    ]);

    started[0].finish();
    await revealed;
    expect(inlineStyleOf(viewer)).toBe("");
  });

  it("does nothing to an element that is already shown", async () => {
    const started = recordAnimations();
    const pill = elementSized();

    await reveal(pill);

    expect(started).toHaveLength(0);
    expect(pill.hidden).toBe(false);
  });
});

describe("hiding an element", () => {
  it("shrinks it to nothing, and only then takes it out of the layout", async () => {
    const started = recordAnimations();
    const pill = elementSized(120);

    const hidden = hide(pill, { axis: "width" });
    await tick();

    expect(pill.hidden).toBe(false);
    expect(started[0].keyframes).toEqual([
      { width: "120px", opacity: 1 },
      { width: "0px", opacity: 0 },
    ]);
    expect(started[0].options.duration).toBe(MOTION_DURATION_MS);

    started[0].finish();
    await hidden;

    expect(pill.hidden).toBe(true);
    expect(inlineStyleOf(pill)).toBe("");
  });

  it("does nothing to an element that is already hidden", async () => {
    const started = recordAnimations();
    const pill = elementHidden();

    await hide(pill);

    expect(started).toHaveLength(0);
    expect(pill.hidden).toBe(true);
  });
});

describe("several elements moving in the same paint", () => {
  it("starts them one beat apart, in the order they were asked for", async () => {
    vi.useFakeTimers();
    const started = recordAnimations();
    const first = elementHidden();
    const second = elementHidden();

    const moving = Promise.all([reveal(first), reveal(second)]);
    await vi.advanceTimersByTimeAsync(0);

    expect(started.map((run) => run.element)).toEqual([first]);

    await vi.advanceTimersByTimeAsync(MOTION_BEAT_MS);

    expect(started.map((run) => run.element)).toEqual([first, second]);

    started.forEach((run) => run.finish());
    await vi.advanceTimersByTimeAsync(MOTION_BEAT_MS);
    await moving;
    expect(first.hidden).toBe(false);
    expect(second.hidden).toBe(false);
  });
});

describe("a reader who asked for less movement", () => {
  it("gets the state change at once, with no animation and no beat", async () => {
    askForLessMotion();
    const started = recordAnimations();
    const pill = elementHidden();
    const other = elementSized();

    const moving = Promise.all([reveal(pill), hide(other)]);

    expect(pill.hidden).toBe(false);
    expect(other.hidden).toBe(true);
    expect(started).toHaveLength(0);

    await moving;
    expect(inlineStyleOf(pill)).toBe("");
    expect(inlineStyleOf(other)).toBe("");
  });
});

describe("a page whose elements cannot animate", () => {
  it("changes them at once through the same calls", async () => {
    const pill = elementHidden();
    const other = elementSized();

    await Promise.all([reveal(pill), hide(other)]);

    expect(pill.hidden).toBe(false);
    expect(other.hidden).toBe(true);
  });
});

describe("a move that is countermanded before it finishes", () => {
  it("cancels the running hide and leaves the element shown", async () => {
    const started = recordAnimations();
    const pill = elementSized(120);

    const hidden = hide(pill);
    await tick();
    expect(started).toHaveLength(1);

    const revealed = reveal(pill);
    await hidden;

    expect(started[0].cancelled).toBe(true);
    expect(pill.hidden).toBe(false);

    await tick();
    expect(started).toHaveLength(2);
    started[1].finish();
    await revealed;

    expect(pill.hidden).toBe(false);
    expect(inlineStyleOf(pill)).toBe("");
  });

  it("answers a second hide with the one already running", async () => {
    const started = recordAnimations();
    const pill = elementSized(120);

    const hidden = hide(pill);
    await tick();
    const again = hide(pill);
    await tick();

    expect(started).toHaveLength(1);

    started[0].finish();
    await Promise.all([hidden, again]);
    expect(pill.hidden).toBe(true);
  });
});

describe("an element the document does not hold", () => {
  it("changes one that was never in it at once, asking for no animation", async () => {
    const started = recordAnimations();
    const loose = document.createElement("span");
    loose.hidden = true;

    await reveal(loose, { axis: "width" });

    expect(started).toHaveLength(0);
    expect(loose.hidden).toBe(false);
    expect(inlineStyleOf(loose)).toBe("");
  });

  it("finishes a move whose element left the document under it", async () => {
    const started = recordAnimations();
    const pill = elementSized(120);

    const hidden = hide(pill, { axis: "width" });
    await tick();
    pill.remove();
    started[0].finish();
    await hidden;

    expect(pill.hidden).toBe(true);
    await expect(motionSettled()).resolves.toBeUndefined();
  });
});

describe("a move the browser refuses to run", () => {
  const animateThatThrows = () => {
    Element.prototype.animate = function animate() {
      throw new Error("no timeline here");
    };
  };

  it("reports the failure, stays out of the way of the next move, and leaves nothing moving", async () => {
    animateThatThrows();
    const pill = elementSized(120);

    await expect(hide(pill, { axis: "width" })).rejects.toThrow("no timeline here");
    expect(pill.hidden).toBe(true);
    expect(inlineStyleOf(pill)).toBe("");
    await expect(motionSettled()).resolves.toBeUndefined();

    const started = recordAnimations();
    const revealed = reveal(pill, { axis: "width" });
    await tick();

    expect(started).toHaveLength(1);
    started[0].finish();
    await revealed;
    expect(pill.hidden).toBe(false);
  });
});

describe("a move countermanded on a page that cannot animate", () => {
  it("leaves no dead record behind for the next move to be answered from", async () => {
    const started = recordAnimations();
    const pill = elementSized(120);

    const hidden = hide(pill, { axis: "width" });
    await tick();
    expect(started).toHaveLength(1);

    stopRecordingAnimations();
    await reveal(pill, { axis: "width" });
    await hidden;
    expect(pill.hidden).toBe(false);

    await hide(pill, { axis: "width" });

    expect(pill.hidden).toBe(true);
  });
});

describe("the hooks a keyed list paints its arrivals and departures with", () => {
  it("grows an entry that arrived from nothing and shrinks one that left", async () => {
    const started = recordAnimations();
    const hooks = motionHooks({ axis: "height" });
    const row = elementSized(200, 40);

    const arriving = hooks.onEnter(row);
    await tick();
    expect(started[0].keyframes).toEqual([
      { height: "0px", opacity: 0 },
      { height: "40px", opacity: 1 },
    ]);
    started[0].finish();
    await arriving;
    expect(row.hidden).toBe(false);

    const leaving = hooks.onExit(row);
    await tick();
    expect(started[1].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
    started[1].finish();
    await leaving;
    expect(row.hidden).toBe(true);
  });
});

describe("settling an element hidden without moving it", () => {
  it("cancels the move it was running and takes it out of the layout at once", async () => {
    const started = recordAnimations();
    const viewer = elementHidden(400, 260);

    const revealed = reveal(viewer, { axis: "height" });
    await tick();
    expect(started).toHaveLength(1);

    settleHidden(viewer);

    expect(started[0].cancelled).toBe(true);
    expect(viewer.hidden).toBe(true);
    expect(inlineStyleOf(viewer)).toBe("");

    await revealed;
    await expect(motionSettled()).resolves.toBeUndefined();
  });

  it("takes one no move is holding out of the layout just the same", () => {
    const pill = elementSized(120);

    settleHidden(pill);

    expect(pill.hidden).toBe(true);
  });
});

describe("an element that has no size to grow, only presence", () => {
  it("fades it in on opacity alone, borrowing no size and no overflow", async () => {
    const started = recordAnimations();
    const scrim = elementHidden(400, 300);

    const revealed = reveal(scrim, { axis: "opacity" });
    await tick();

    expect(scrim.hidden).toBe(false);
    expect(started[0].keyframes).toEqual([{ opacity: 0 }, { opacity: 1 }]);
    expect(scrim.style.overflow).toBe("");

    started[0].finish();
    await revealed;

    expect(scrim.hidden).toBe(false);
    expect(inlineStyleOf(scrim)).toBe("");
  });

  it("fades it out, and only then takes it out of the layout", async () => {
    const started = recordAnimations();
    const scrim = elementSized(400, 300);

    const hidden = hide(scrim, { axis: "opacity" });
    await tick();

    expect(scrim.hidden).toBe(false);
    expect(started[0].keyframes).toEqual([{ opacity: 1 }, { opacity: 0 }]);

    started[0].finish();
    await hidden;

    expect(scrim.hidden).toBe(true);
    expect(inlineStyleOf(scrim)).toBe("");
  });
});
