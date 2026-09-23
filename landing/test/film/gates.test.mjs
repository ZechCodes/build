import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createGates } from "../../src/film/gates.js";

function recorder() {
  const log = [];
  const gates = createGates();
  const add = (time, name) => gates.add(time, (instant) => log.push(`${name}+${instant ? "!" : ""}`), () => log.push(`${name}-`));
  return { log, gates, add };
}

describe("gates on the scrubbed clock", () => {
  it("fires each gate once per crossing, forward in order and backward in reverse", () => {
    const { log, gates, add } = recorder();
    add(30, "b"); add(10, "a"); add(50, "c");
    gates.update(0);
    gates.update(5);
    gates.update(35);
    gates.update(35);
    gates.update(20);
    gates.update(60);
    gates.update(0);
    assert.deepEqual(log, ["a+", "b+", "b-", "b+", "c+", "c-", "b-", "a-"]);
  });

  it("counts a gate exactly at the playhead as passed", () => {
    const { log, gates, add } = recorder();
    add(10, "a");
    gates.update(0);
    gates.update(10);
    gates.update(10);
    gates.update(9.99);
    assert.deepEqual(log, ["a+", "a-"]);
  });

  it("opens mid-film with every earlier gate in its forward state, without motion", () => {
    const { log, gates, add } = recorder();
    add(10, "a"); add(30, "b"); add(50, "c");
    gates.update(40);
    assert.deepEqual(log, ["a+!", "b+!"]);
    gates.update(55);
    assert.deepEqual(log, ["a+!", "b+!", "c+"]);
  });

  it("tolerates a gate with no backward action", () => {
    const gates = createGates();
    const log = [];
    gates.add(10, () => log.push("on"));
    gates.update(0);
    gates.update(20);
    gates.update(0);
    assert.deepEqual(log, ["on"]);
  });
});
