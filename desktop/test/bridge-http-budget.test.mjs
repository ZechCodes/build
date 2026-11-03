import assert from "node:assert/strict";
import test from "node:test";
import { createWireBudget } from "../src/bridge-http-budget.mjs";
const M = 1024 * 1024;

test("complete route reservations bound combined wire bytes before any input/output buffering", () => {
  const budget = createWireBudget();
  const release = [];
  for (let i = 0; i < 16; i++)
    release.push(budget.reserve({ requestMax: M, responseMax: 8 * M }));
  assert.throws(() => budget.reserve({ requestMax: M, responseMax: 8 * M }), {
    code: "busy",
  });
  release[0]();
  release[0]();
  const final = budget.reserve({ requestMax: M, responseMax: 8 * M });
  for (const done of release) done();
  final();
  assert.doesNotThrow(() =>
    budget.reserve({ requestMax: M, responseMax: 8 * M }),
  );
});

test("bulk subbudget and opening/global limits preserve small request headroom", () => {
  const budget = createWireBudget();
  const opening = {};
  const one = budget.reserve({
    requestMax: 2 * M,
    responseMax: 2 * M,
    opening,
  });
  assert.throws(
    () => budget.reserve({ requestMax: 2 * M, responseMax: 2 * M, opening }),
    { code: "busy" },
  );
  const two = budget.reserve({
    requestMax: 2 * M,
    responseMax: 2 * M,
    opening: {},
  });
  assert.throws(
    () =>
      budget.reserve({ requestMax: 2 * M, responseMax: 2 * M, opening: {} }),
    { code: "busy" },
  );
  one();
  two();
  const bulk = budget.reserve({
    requestMax: 7 * M,
    responseMax: 8 * M,
    opening,
  });
  assert.throws(
    () =>
      budget.reserve({ requestMax: 7 * M, responseMax: 8 * M, opening: {} }),
    { code: "busy" },
  );
  const small = budget.reserve({ requestMax: M, responseMax: 8 * M });
  small();
  bulk();
});


test("family and opening caps leave half the full reply pool for another client", () => {
  const budget = createWireBudget();
  const opening = {};
  const reserve = (family, heldOpening = opening) => budget.reserve({
    requestMax: M, responseMax: 8 * M, opening: heldOpening, family,
  });
  const held = Array.from({length: 16}, () => reserve("a"));
  assert.throws(() => reserve("a", {}), { code: "busy" });
  assert.throws(() => reserve("b"), { code: "busy" });
  const second = {};
  const others = Array.from({length: 16}, () => reserve("b", second));
  assert.throws(() => reserve("c", {}), { code: "busy" });
  held.forEach(done => done());
  others.forEach(done => done());
  assert.doesNotThrow(() => reserve("a"));
});
