// #434: a bridge says `harnesses.changed` with the revision its inventory
// moved to. It is about the machine, so it is routed whether or not that
// machine's change subscriptions are armed, and only to that machine.
import { expect, it } from "vitest";
import { dispatchChangeEvent } from "../src/core/changeEvents.js";
import { onHarnessesChanged } from "../src/core/harnessInventoryEvents.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

it("routes a machine's inventory revision to that machine's listeners alone", () => {
  const heardA = [];
  const heardB = [];
  const stopA = onHarnessesChanged("dev-a", (revision) => heardA.push(revision));
  const stopB = onHarnessesChanged("dev-b", (revision) => heardB.push(revision));

  expect(dispatchChangeEvent({ type: "harnesses.changed", revision: 7 }, "dev-a")).toBe(true);

  expect(heardA).toEqual([7]);
  expect(heardB).toEqual([]);
  stopA();
  dispatchChangeEvent({ type: "harnesses.changed", revision: 8 }, "dev-a");
  expect(heardA).toEqual([7]);
  stopB();
});

it("ignores a push without a usable revision", () => {
  const heard = [];
  const stop = onHarnessesChanged("dev-a", (revision) => heard.push(revision));
  dispatchChangeEvent({ type: "harnesses.changed" }, "dev-a");
  dispatchChangeEvent({ type: "harnesses.changed", revision: "9" }, "dev-a");
  dispatchChangeEvent({ type: "harnesses.changed", revision: -1 }, "dev-a");
  expect(heard).toEqual([]);
  stop();
});

it("is a push the v1 adapter knows", () => {
  expect(v1.parseEvent({ type: "harnesses.changed", revision: 1 })).not.toBe(null);
});
