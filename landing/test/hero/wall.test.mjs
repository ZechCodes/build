// The notification wall (#316): slots packed into the field (all of it, or
// a dome at the top), each cycling its own notifications in and out fast,
// and an exit that blurs the wall out from the laptop's screen.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  NOTE_STATES, WALL_EASE, noteAt, noteFrames, planSlot, requestSlots, slotCycles, wallShape, wallSlots,
} from "../../src/lab/legacy/wall.js";

const SIZES = {
  phone: { width: 390, height: 844, narrow: true, clear: 127 },
  laptop: { width: 1440, height: 900, narrow: false, clear: 199 },
  wide: { width: 1920, height: 1080, narrow: false, clear: 239 },
  huge: { width: 2560, height: 1440, narrow: false, clear: 318 },
};

const slotsAt = (size, shape) => wallSlots({ ...SIZES[size], shape: wallShape({ ...SIZES[size], kind: shape }) });
const reachOf = (shape, { x, y }) => Math.hypot((x - shape.cx) / shape.rx, y / shape.ry);

describe("the wall's slots", () => {
  it("cover the whole field when full, densely, none overlapping in its row", () => {
    const { width, height } = SIZES.laptop;
    const slots = slotsAt("laptop", "full");
    assert.ok(slots.length > 150, `${slots.length} slots`);
    assert.ok(slots.some((slot) => slot.y > height * 0.9) && slots.some((slot) => slot.x > width * 0.9));
    const rows = Map.groupBy(slots, (slot) => slot.row);
    for (const row of rows.values()) {
      const sorted = row.toSorted((a, b) => a.x - b.x);
      sorted.slice(1).forEach((slot, index) => assert.ok(slot.x - slot.width / 2 >= sorted[index].x + sorted[index].width / 2, "apart"));
    }
  });

  it("stand only inside the dome, above the headline and the laptop, thinning towards its rim", () => {
    for (const size of ["phone", "laptop", "wide", "huge"]) {
      const shape = wallShape({ ...SIZES[size], kind: "dome" });
      const slots = slotsAt(size, "dome");
      for (const slot of slots) {
        assert.ok(reachOf(shape, slot) < shape.fade, `${size}: inside what the mask shows`);
        assert.ok(slot.y + slot.height / 2 <= SIZES[size].clear, `${size}: above the clear line`);
      }
      const core = slots.filter((slot) => reachOf(shape, slot) < shape.inner).length;
      const rim = slots.length - core;
      const area = (from, to) => to ** 2 - from ** 2;
      assert.ok(rim / area(shape.inner, shape.fade) < (core / area(0, shape.inner)) * 0.8, `${size}: sparser at the rim`);
    }
  });

  it("keep about as many pills in the dome on a wide screen as on a desktop one", () => {
    const counts = ["laptop", "wide", "huge"].map((size) => slotsAt(size, "dome").length);
    assert.ok(Math.max(...counts) / Math.min(...counts) < 1.35, counts.join(", "));
    assert.ok(slotsAt("laptop", "dome").length < slotsAt("laptop", "full").length * 0.4);
  });

  it("make a dense wall across a phone's top", () => {
    const slots = slotsAt("phone", "dome");
    const top = slots.filter((slot) => slot.y < SIZES.phone.clear * 0.5);
    const covered = top.reduce((sum, slot) => sum + slot.width, 0);
    assert.ok(covered > SIZES.phone.width * 1.2, `${Math.round(covered)} px of pills across the top half`);
  });

  it("give each slot two different notifications", () => {
    for (const slot of slotsAt("laptop", "dome")) {
      assert.equal(slot.notes.length, 2);
      assert.notEqual(slot.notes[0].text, slot.notes[1].text);
    }
  });

  it("are the same on every build", () => {
    assert.deepEqual(slotsAt("laptop", "dome"), slotsAt("laptop", "dome"));
  });
});

describe("a slot's cycle", () => {
  const slots = slotsAt("laptop", "full");
  const plans = slots.map((slot, index) => planSlot(index));

  it("keeps the wall moving everywhere: at every moment some slide in, some show and some blur out", () => {
    for (let t = 0; t < 6; t += 0.1) {
      const states = plans.flatMap((plan) => [0, 1].map((note) => noteAt(plan, note, t)));
      const share = (state) => states.filter((value) => value === state).length / plans.length;
      assert.ok(share("in") > 0.05 && share("shown") > 0.3 && share("out") > 0.1, `t=${t.toFixed(1)}`);
      const empty = plans.filter((plan) => noteAt(plan, 0, t) === null && noteAt(plan, 1, t) === null).length;
      assert.ok(empty / plans.length < 0.25, `t=${t.toFixed(1)}: ${empty} empty`);
    }
  });

  it("is quick: a notification is in and gone within about a second and a half", () => {
    for (const plan of plans) {
      for (const note of plan.notes) assert.ok(note.in + note.hold + note.out < 1.6);
    }
  });

  it("stops filling at its exit: what is shown then blurs out, and nothing comes after", () => {
    const plan = planSlot(7);
    const exit = 1.4;
    for (const note of [0, 1]) {
      const cycles = slotCycles(plan, note, { exit });
      assert.ok(cycles.every((cycle) => cycle.start < exit));
      const last = cycles.at(-1);
      assert.ok(last.start + last.in + last.hold <= Math.max(exit, last.start + last.in) + 1e-9);
    }
    const gone = exit + Math.max(...plan.notes.map((note) => note.in + note.out));
    assert.equal(noteAt(plan, 0, gone + 0.01, exit), null);
    assert.equal(noteAt(plan, 1, gone + 0.01, exit), null);
  });

  it("names every state it passes through", () => {
    assert.deepEqual(NOTE_STATES, ["in", "shown", "out"]);
  });
});

describe("a notification's keyframes", () => {
  const plan = planSlot(3);
  const peak = 0.8;

  it("animate transform and opacity, and the filter only when the blur is a filter", () => {
    for (const blur of ["copy", "filter"]) {
      const { sharp, blurred } = noteFrames(slotCycles(plan, 0, { exit: 2 }), { span: 4, peak, blur });
      const keys = new Set([...sharp, ...(blurred ?? [])].flatMap((frame) => Object.keys(frame)));
      const allowed = ["offset", "easing", "transform", "opacity", ...(blur === "filter" ? ["filter"] : [])];
      for (const key of keys) assert.ok(allowed.includes(key), `${blur}: ${key}`);
      assert.equal(blurred === null, blur === "filter");
    }
  });

  it("run from hidden to shown at the slot's peak and back to hidden, offsets in order", () => {
    const { sharp, blurred } = noteFrames(slotCycles(plan, 0, { exit: 2 }), { span: 4, peak, blur: "copy" });
    for (const frames of [sharp, blurred]) {
      frames.slice(1).forEach((frame, index) => assert.ok(frame.offset >= frames[index].offset));
      assert.equal(frames[0].offset, 0);
      assert.equal(frames.at(-1).offset, 1);
      assert.equal(Number(frames.at(-1).opacity), 0);
      for (const frame of frames) assert.ok(Number(frame.opacity) <= peak + 1e-9);
    }
    assert.ok(sharp.some((frame) => Number(frame.opacity) === peak));
    assert.ok(Math.max(...blurred.map((frame) => Number(frame.opacity))) > 0.5 * peak);
  });

  it("loop over the slot's period when endless", () => {
    const cycles = slotCycles(plan, 1, { exit: Infinity });
    assert.equal(cycles.length, 1);
    assert.equal(cycles[0].start, 0);
    const { sharp } = noteFrames(cycles, { span: plan.period, peak, blur: "filter" });
    assert.equal(sharp[0].offset, 0);
    assert.equal(sharp.at(-1).offset, 1);
    assert.ok(sharp.some((frame) => /blur\([1-9]/.test(frame.filter)));
  });

  it("slide in on an ease out and blur out on an ease in", () => {
    assert.match(WALL_EASE.in, /^cubic-bezier\(/);
    assert.match(WALL_EASE.out, /^cubic-bezier\(/);
  });
});

describe("the requests' slots", () => {
  it("are three, inside the dome, apart, above the clear line", () => {
    const shape = wallShape({ ...SIZES.laptop, kind: "dome" });
    const slots = slotsAt("laptop", "dome");
    const chosen = requestSlots(slots, shape, SIZES.laptop);
    assert.equal(chosen.length, 3);
    assert.equal(new Set(chosen).size, 3);
    for (const slot of chosen) assert.ok(reachOf(shape, slot) < shape.inner);
  });

  it("are three in the full wall too", () => {
    const shape = wallShape({ ...SIZES.phone, kind: "full" });
    assert.equal(requestSlots(slotsAt("phone", "full"), shape, SIZES.phone).length, 3);
  });
});
