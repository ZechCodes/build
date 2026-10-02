import assert from "node:assert/strict";
import test from "node:test";

import { summarizeChecks } from "./checkResults.mjs";

test("all seventeen passing checks produce a successful exit status", () => {
  const results = Array.from({ length: 17 }, () => ({ ok: true }));

  assert.deepEqual(summarizeChecks(results), { passed: 17, total: 17, exitCode: 0 });
});

test("a failed check anywhere in the run produces exit status 1", () => {
  for (let failedAt = 0; failedAt < 17; failedAt++) {
    const results = Array.from({ length: 17 }, (_, index) => ({ ok: index !== failedAt }));

    assert.deepEqual(summarizeChecks(results), { passed: 16, total: 17, exitCode: 1 });
  }
});

test("multiple failures still report the number of checks that passed", () => {
  const results = [{ ok: false }, { ok: true }, { ok: false }];

  assert.deepEqual(summarizeChecks(results), { passed: 1, total: 3, exitCode: 1 });
  assert.deepEqual(summarizeChecks([{ ok: false }]), { passed: 0, total: 1, exitCode: 1 });
});

test("the tally uses the same truthiness as the per-check PASS/FAIL output", () => {
  const results = [true, 1, "present", false, 0, "", null, undefined].map((ok) => ({ ok }));

  assert.deepEqual(summarizeChecks(results), { passed: 3, total: 8, exitCode: 1 });
});

test("an empty tally has no failed checks", () => {
  assert.deepEqual(summarizeChecks([]), { passed: 0, total: 0, exitCode: 0 });
});
