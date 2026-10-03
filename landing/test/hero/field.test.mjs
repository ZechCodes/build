// The notification field is data: seeded, so every build draws the same
// field, and limited to the harnesses Build supports.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import {
  ATTENTION,
  HARNESS_NAMES,
  LANE_TIERS,
  ROUTINE_EVENTS,
  SUPPORTED_HARNESSES,
  DRIFT_REACH,
  createField,
  pillWidthVw,
} from "../../src/lab/legacy/field.js";

const pillsOf = (field) => field.lanes.flatMap((lane) => [...lane.before, ...lane.after]);

describe("the notification field", () => {
  it("draws the same field from the same seed, and another from another", () => {
    assert.deepEqual(createField(), createField());
    assert.notDeepEqual(createField({ seed: 7 }), createField());
  });

  it("is a flood: 22 to 26 lanes on a desktop and 16 to 20 on a phone", () => {
    const { lanes } = createField();
    assert.ok(lanes.length >= 22 && lanes.length <= 26, `${lanes.length} lanes`);
    const narrow = lanes.filter((lane) => lane.narrow);
    assert.ok(narrow.length >= 16 && narrow.length <= 20, `${narrow.length} narrow lanes`);
    assert.deepEqual(narrow.map((lane) => lane.narrowRow), narrow.map((_, index) => index));
  });

  it("mixes fast faint lanes with slow sharp ones, at no two speeds alike", () => {
    const { lanes } = createField();
    const tiers = new Set(lanes.map((lane) => lane.tier));
    assert.deepEqual([...tiers].sort(), Object.keys(LANE_TIERS).sort());
    for (const lane of lanes) {
      const [low, high] = LANE_TIERS[lane.tier].drift;
      assert.ok(Math.abs(lane.drift) >= low && Math.abs(lane.drift) <= high, `lane ${lane.index} drift ${lane.drift}`);
    }
    const speeds = lanes.map((lane) => lane.drift);
    assert.ok(new Set(speeds).size === speeds.length, "no two lanes at one speed");
    const anchors = lanes.map((lane) => lane.anchor);
    assert.ok(new Set(anchors).size === anchors.length, "no synchronized procession");
  });

  it("moves fast: the far lanes cross the window more than twice while it plays", () => {
    for (const lane of createField().lanes.filter((candidate) => candidate.tier === "far")) {
      assert.ok(Math.abs(lane.drift) >= 140, `lane ${lane.index} drift ${lane.drift}`);
    }
  });

  it("runs cross traffic, with every request in a lane moving right", () => {
    const { lanes } = createField();
    const leftward = lanes.filter((lane) => lane.drift < 0);
    assert.ok(leftward.length >= 6 && leftward.length <= lanes.length / 2, `${leftward.length} lanes run left`);
    assert.ok(leftward.some((lane) => lane.narrow), "a phone sees cross traffic too");
    for (const lane of lanes.filter((candidate) => candidate.after[0]?.attention)) assert.ok(lane.drift > 0, `lane ${lane.index}`);
  });

  it("varies the size of its lanes in depth: far small, near large, the requests at full size", () => {
    const { lanes } = createField();
    for (const lane of lanes.filter((candidate) => !candidate.after[0]?.attention)) {
      const [low, high] = LANE_TIERS[lane.tier].size;
      assert.ok(lane.size >= low && lane.size <= high, `lane ${lane.index} size ${lane.size}`);
    }
    assert.ok(LANE_TIERS.far.size[1] < LANE_TIERS.mid.size[0] && LANE_TIERS.mid.size[1] <= LANE_TIERS.near.size[0]);
    for (const lane of lanes.filter((candidate) => candidate.after[0]?.attention)) assert.equal(lane.size, 1);
  });

  it("jostles every routine lane up and down, out of step, and holds the requests' lanes still", () => {
    const { lanes } = createField();
    for (const lane of lanes) {
      if (lane.after[0]?.attention) {
        assert.equal(lane.sway, 0, `lane ${lane.index} carries a request`);
        continue;
      }
      assert.ok(lane.sway >= 3 && lane.sway <= 9, `lane ${lane.index} sway ${lane.sway}`);
      assert.ok(lane.swaySeconds >= 0.4 && lane.swaySeconds <= 1.2, `lane ${lane.index} sway period ${lane.swaySeconds}`);
    }
    const periods = lanes.filter((lane) => lane.sway).map((lane) => lane.swaySeconds);
    assert.ok(new Set(periods).size > periods.length / 2, "the lanes do not jostle in step");
  });

  it("carries enough pills in each lane to stay full while it drifts, and no more than the DOM can bear", () => {
    const covered = (pills, size) => pills.reduce((sum, pill) => sum + pillWidthVw(pill.text, size) + Math.max(0, pill.gap) / 14.4, 0);
    let total = 0;
    for (const lane of createField().lanes) {
      // The side the lane comes from also carries everything that drifts in
      // before the drift comes to rest.
      const incoming = Math.abs(lane.drift) * DRIFT_REACH;
      const [left, right] = [lane.anchor * 100, (1 - lane.anchor) * 100];
      const [needBefore, needAfter] = lane.drift > 0 ? [left + incoming, right] : [left, right + incoming];
      assert.ok(covered(lane.before, lane.size) >= needBefore, `lane ${lane.index} before`);
      assert.ok(covered(lane.after, lane.size) >= needAfter, `lane ${lane.index} after`);
      total += lane.before.length + lane.after.length;
    }
    // One element a pill; main's calmer field had 304 pills of three each.
    assert.ok(total <= 480, `${total} pills`);
  });

  it("brings its drift to rest where hero.css does, after the entrance has taken it over", () => {
    const css = readFileSync(new URL("../../src/lab/legacy/hero.css", import.meta.url), "utf8");
    const keyframes = css.slice(css.indexOf("@keyframes hero-drift"), css.indexOf("@keyframes hero-sway"));
    const [last] = [...keyframes.matchAll(/to\s*\{\s*transform:\s*translateX\(calc\(var\(--drift\) \* var\(--drift-scale\) \* ([\d.]+)\)\)/g)].map((match) => Number(match[1]));
    assert.equal(last, DRIFT_REACH, "the lanes come to rest at DRIFT_REACH of their drift");
    // Steady until the entrance would have taken over, then braking.
    const [, at, share] = keyframes.match(/(\d+)%\s*\{\s*transform:\s*translateX\(calc\(var\(--drift\) \* var\(--drift-scale\) \* ([\d.]+)\)\)/);
    assert.equal(Number(share), Number(at) / 100, "linear up to the brake");
    assert.ok(DRIFT_REACH >= 0.5 && DRIFT_REACH < 0.7, `${DRIFT_REACH}`);
  });

  it("estimates a pill's width from its words and its lane's size", () => {
    assert.ok(pillWidthVw("Lint clean", 1) < pillWidthVw("Checking dependencies", 1));
    assert.ok(pillWidthVw("Lint clean", 0.8) < pillWidthVw("Lint clean", 1));
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
      // A 200px pill on a 390px phone still ends inside the window.
      assert.ok(lane.narrowAnchor >= 0.1 && lane.narrowAnchor + 200 / 390 <= 0.95, `${entry.id} on a phone`);
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
