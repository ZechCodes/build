import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  ACTS,
  COPY_IN,
  COPY_OUT,
  SCENES,
  SCENE_SCREENS,
  SCREEN_CUES,
  TOTAL_TRAVEL,
  actAt,
  actStart,
  at,
  entrancePose,
  fullPose,
  POSES,
  sceneClock,
  sceneSeconds,
  span,
} from "../../src/film/acts.js";
import { createScreenResolver, cueAt, upcomingCues } from "../../src/film/cues.js";

describe("the film's clock", () => {
  it("runs eight acts for about 930 viewport heights, in order", () => {
    assert.equal(ACTS.length, 8);
    assert.deepEqual(ACTS.map((act) => act.id), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert.equal(TOTAL_TRAVEL, 930);
    assert.equal(actStart(1), 0);
    assert.equal(actStart(2), 110);
    assert.equal(actStart(8), 840);
  });

  it("places a local beat inside its act", () => {
    assert.equal(at(1, 0.5), 55);
    assert.equal(at(3, 0.2), 224);
    assert.equal(span(2, 0.25, 0.45), 18);
  });

  it("reads an act and local progress back from a position", () => {
    assert.deepEqual(actAt(0), { act: 1, local: 0 });
    assert.deepEqual(actAt(150), { act: 2, local: 40 / 90 });
    assert.deepEqual(actAt(930), { act: 8, local: 1 });
    assert.deepEqual(actAt(-5), { act: 1, local: 0 });
  });

  it("keeps a hold in every act between the scene's arrival and the copy's exit", () => {
    for (const act of ACTS) {
      const arrive = SCENES[act.id]?.arrive ?? COPY_IN;
      const hold = act.length * (COPY_OUT - arrive);
      assert.ok(hold >= 55, `act ${act.id} holds ${hold} units after arrival`);
    }
  });

  it("orders each device's display cues by time", () => {
    for (const cues of Object.values(SCREEN_CUES)) {
      const times = cues.map(([time]) => time);
      assert.deepEqual(times, [...times].sort((a, b) => a - b));
    }
  });
});

describe("a scene's clock", () => {
  it("starts at the act's arrival and runs the storyboard's beats in seconds", () => {
    const clock = sceneClock(5);
    assert.equal(clock.at(5, SCENES[5].arrive), 0);
    assert.equal(clock.at(5, 0.1), 0);
    assert.ok(Math.abs(clock.at(5, 1) - SCENES[5].seconds) < 1e-9);
    assert.ok(clock.at(5, 0.6) < clock.at(5, 0.74));
    assert.ok(Math.abs(clock.span(5, 0.6, 0.74) - (clock.at(5, 0.74) - clock.at(5, 0.6))) < 1e-9);
  });

  it("gives act 7's open finding at least four seconds before the approval", () => {
    const clock = sceneClock(7);
    assert.ok(clock.at(7, 0.66) - clock.at(7, 0.55) >= 4, `${clock.at(7, 0.66) - clock.at(7, 0.55)}s`);
    assert.ok(Math.abs(clock.span(7, 0.55, 0.66) - (clock.at(7, 0.66) - clock.at(7, 0.55))) < 1e-9);
    assert.ok(Math.abs(clock.span(7, 0.7, 0.8) - (clock.at(7, 0.8) - clock.at(7, 0.7))) < 1e-9, "a span past the hold is not stretched");
    assert.ok(Math.abs(sceneSeconds(7) - (SCENES[7].seconds + 2.7)) < 1e-9);
  });

  it("measures a duration from before the arrival at full length", () => {
    assert.ok(sceneClock(2).span(2, 0, 0.08) > 0);
  });

  it("holds the phone's question for two seconds before its answer", () => {
    const clock = sceneClock(4);
    assert.ok(clock.at(4, 0.66) - clock.at(4, 0.45) >= 2);
  });

  it("refuses another act's beats and an act without a scene", () => {
    assert.throws(() => sceneClock(5).at(6, 0.5));
    assert.throws(() => sceneClock(8));
  });

  it("arrives inside its act, before the copy leaves", () => {
    for (const [actId, scene] of Object.entries(SCENES)) {
      assert.ok(scene.arrive > 0 && scene.arrive < COPY_OUT, `act ${actId}`);
      assert.ok(scene.seconds > 0 && scene.seconds < 12, `act ${actId} runs ${scene.seconds}s`);
    }
  });

  it("names the displays a scene swaps so they can be preloaded, and keeps them off the scroll cues", () => {
    const scrollNames = Object.values(SCREEN_CUES).flat().map(([, name]) => name);
    for (const devices of Object.values(SCENE_SCREENS)) {
      for (const names of Object.values(devices)) {
        for (const name of names) assert.ok(!scrollNames.includes(name) || name.startsWith("ui05-merged"), name);
      }
    }
    assert.deepEqual(SCENE_SCREENS[4].phone, ["ui03-answer-iphone", "ui03-resumed-iphone"]);
  });
});

describe("poses", () => {
  it("keeps act 4's pair close, the phone toward the middle", () => {
    const gap = POSES.phone[4].x - POSES.laptop[4].x;
    assert.ok(gap <= 32, `centres ${gap}% apart`);
    assert.ok(POSES.phone[4].x <= 70);
  });

  it("raises the hero a little above centre", () => {
    assert.ok(POSES.laptop[1].y <= 54);
  });

  it("fills every field so a tween has a target for each", () => {
    const pose = fullPose({ x: 65, y: 57, w: 52, pitch: 4 });
    assert.deepEqual(pose, { x: 65, y: 57, w: 52, yaw: 0, pitch: 4, roll: 0, opacity: 1, lidOpen: 1, faceCamera: 1 });
    assert.equal(fullPose({}).opacity, 0);
  });

  it("starts a remote screen offset, smaller, turned and transparent", () => {
    const settled = { x: 83, y: 56, w: 21, yaw: -12, pitch: 2, roll: -2 };
    const entrance = entrancePose("phone", settled);
    assert.equal(entrance.x, 96);
    assert.equal(entrance.y, 66);
    assert.ok(Math.abs(entrance.w - 15.12) < 1e-9);
    assert.equal(entrance.yaw, 16);
    assert.equal(entrance.opacity, 0);
  });

  it("only fades a device with no entrance", () => {
    assert.deepEqual(entrancePose("laptop", { x: 1, y: 2, w: 3 }), { x: 1, y: 2, w: 3, opacity: 0 });
  });
});

describe("cues", () => {
  const cues = [[10, "a"], [20, "b"], [30, "c"]];

  it("resolves to the last cue at or before the time, or nothing", () => {
    assert.equal(cueAt(cues, 5), null);
    assert.equal(cueAt(cues, 10), "a");
    assert.equal(cueAt(cues, 25), "b");
    assert.equal(cueAt(cues, 99), "c");
  });

  it("lists what is coming within the lookahead", () => {
    assert.deepEqual(upcomingCues(cues, 12, 10), ["b"]);
    assert.deepEqual(upcomingCues(cues, 12, 30), ["b", "c"]);
  });

  it("applies a display once per change, in either scroll direction", () => {
    const applied = [];
    const resolve = createScreenResolver({ laptop: cues }, (device, value) => applied.push(`${device}:${value}`));
    resolve(12); resolve(15); resolve(25); resolve(12); resolve(5);
    assert.deepEqual(applied, ["laptop:a", "laptop:b", "laptop:a"]);
  });

  it("lets a scene's display win inside its act and the scroll cues resume after", () => {
    const applied = [];
    const overrides = new Map();
    const resolve = createScreenResolver({ laptop: cues }, (device, value) => applied.push(value), overrides);
    resolve(12);
    overrides.set("laptop", { name: "scene", until: 20 });
    resolve(15);
    resolve(25);
    overrides.delete("laptop");
    resolve(12);
    assert.deepEqual(applied, ["a", "scene", "b", "a"]);
  });
});
