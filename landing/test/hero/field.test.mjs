// The notification field is data: seeded, so every build draws the same
// field, and limited to the harnesses Build supports.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  ATTENTION,
  HARNESS_NAMES,
  LANE_TIERS,
  ROUTINE_EVENTS,
  SUPPORTED_HARNESSES,
  createField,
} from "../../src/hero/field.js";

const pillsOf = (field) => field.lanes.flatMap((lane) => [...lane.before, ...lane.after]);

describe("the notification field", () => {
  it("draws the same field from the same seed, and another from another", () => {
    assert.deepEqual(createField(), createField());
    assert.notDeepEqual(createField({ seed: 7 }), createField());
  });

  it("has 10 to 14 lanes on a desktop and 6 to 8 on a phone", () => {
    const { lanes } = createField();
    assert.ok(lanes.length >= 10 && lanes.length <= 14, `${lanes.length} lanes`);
    const narrow = lanes.filter((lane) => lane.narrow);
    assert.ok(narrow.length >= 6 && narrow.length <= 8, `${narrow.length} narrow lanes`);
    assert.deepEqual(narrow.map((lane) => lane.narrowRow), narrow.map((_, index) => index));
  });

  it("mixes fast faint lanes with slow sharp ones, every lane moving right", () => {
    const { lanes } = createField();
    const tiers = new Set(lanes.map((lane) => lane.tier));
    assert.deepEqual([...tiers].sort(), Object.keys(LANE_TIERS).sort());
    for (const lane of lanes) {
      assert.ok(lane.drift > 0, "left to right");
      const [low, high] = LANE_TIERS[lane.tier].drift;
      assert.ok(lane.drift >= low && lane.drift <= high, `lane ${lane.index} drift ${lane.drift}`);
    }
    const speeds = lanes.map((lane) => lane.drift);
    assert.ok(new Set(speeds).size === speeds.length, "no two lanes at one speed");
    const anchors = lanes.map((lane) => lane.anchor);
    assert.ok(new Set(anchors).size === anchors.length, "no synchronized procession");
  });

  it("carries enough pills in each lane to stay full while it drifts", () => {
    for (const lane of createField().lanes) {
      // The narrowest pill with its gap is 7vw on a 1920 window; a lane must
      // cover what is left of its anchor plus everything that drifts in.
      assert.ok(lane.before.length * 7 >= lane.anchor * 100 + lane.drift, `lane ${lane.index} before`);
      assert.ok(lane.after.length * 7 >= (1 - lane.anchor) * 100, `lane ${lane.index} after`);
    }
  });

  it("names only supported harnesses, and the list is the caller's", () => {
    assert.deepEqual([...SUPPORTED_HARNESSES], ["claude", "codex", "pi"]);
    for (const pill of pillsOf(createField())) assert.ok(SUPPORTED_HARNESSES.includes(pill.harness), pill.harness);
    const two = pillsOf(createField({ harnesses: ["claude", "codex"] }));
    assert.ok(two.every((pill) => ["claude", "codex"].includes(pill.harness)));
    for (const id of ["opencode", "gemini", "cursor"]) assert.ok(HARNESS_NAMES[id], `${id} is ready to be added`);
    assert.throws(() => createField({ harnesses: ["copilot"] }), /copilot/);
    assert.throws(() => createField({ harnesses: [] }), /harness/);
  });

  it("has exactly three attention requests, with stable identities", () => {
    const attention = pillsOf(createField()).filter((pill) => pill.attention);
    assert.deepEqual(attention.map((pill) => pill.attention).sort(), ["approval", "question", "review"]);
    assert.deepEqual(ATTENTION.map((entry) => entry.text), ["Review ready", "Needs your approval", "Which approach?"]);
    for (const entry of ATTENTION) {
      const pill = attention.find((candidate) => candidate.attention === entry.id);
      assert.equal(pill.text, entry.text);
      assert.equal(pill.key, `attention-${entry.id}`);
    }
    const routine = pillsOf(createField()).filter((pill) => !pill.attention).map((pill) => pill.text);
    for (const entry of ATTENTION) assert.ok(!routine.includes(entry.text), `${entry.text} is only an attention pill`);
  });

  it("starts each attention request where it can be read, in a sharp lane shown on phones too", () => {
    const { lanes } = createField();
    for (const entry of ATTENTION) {
      const lane = lanes.find((candidate) => candidate.after[0]?.attention === entry.id);
      assert.ok(lane, entry.id);
      assert.equal(lane.tier, "near");
      assert.ok(lane.narrow, `${entry.id} lane shows on a phone`);
      assert.ok(lane.anchor > 0.3 && lane.anchor < 0.7, `${entry.id} anchor ${lane.anchor}`);
    }
  });

  it("varies its phrasing: no lane repeats a message back to back, none dominates", () => {
    const { lanes } = createField();
    for (const lane of lanes) {
      const texts = [...[...lane.before].reverse(), ...lane.after].map((pill) => pill.text);
      texts.slice(1).forEach((text, index) => assert.notEqual(text, texts[index], `lane ${lane.index}`));
    }
    const counts = new Map();
    for (const pill of pillsOf(createField())) counts.set(pill.text, (counts.get(pill.text) || 0) + 1);
    const total = pillsOf(createField()).length;
    for (const [text, count] of counts) assert.ok(count / total < 0.06, `${text} is ${count} of ${total}`);
    assert.ok(ROUTINE_EVENTS.length >= 30);
    for (const brief of ["Reading auth.ts", "Updating tests", "Tests passed", "Planning next step", "Checking dependencies", "Running build", "Changes ready"]) {
      assert.ok(ROUTINE_EVENTS.includes(brief), brief);
    }
  });

  it("gives every pill a unique key", () => {
    const keys = pillsOf(createField()).map((pill) => pill.key);
    assert.equal(new Set(keys).size, keys.length);
  });
});
