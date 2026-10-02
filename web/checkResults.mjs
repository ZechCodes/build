/** Tally the same truthy/falsy results printed as PASS/FAIL by the checks. */
export function summarizeChecks(results) {
  const passed = results.filter((result) => result.ok).length;
  const total = results.length;
  return { passed, total, exitCode: passed === total ? 0 : 1 };
}
