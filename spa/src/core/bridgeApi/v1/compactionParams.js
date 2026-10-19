const setters = new Set(["project.set_compaction", "workspace.set_compaction"]);
export const needsCompactionSupport = (method, params) => setters.has(method) || Object.hasOwn(params || {}, "compaction_thresholds");
const invalidThreshold = (value) => value !== null && (!Number.isSafeInteger(value) || value <= 0);

/** Refuse new write shapes locally on an old greeting, never silently drop a
 * requested policy. Legacy positive/null conversation limits retain their wire. */
export function compactionWriteError(method, params, supported) {
  params = params && typeof params === "object" ? params : {};
  const thresholds = params.compaction_thresholds;
  const values = ["compact_above_tokens", "max_context_tokens"]
    .filter((field) => Object.hasOwn(params, field)).map((field) => params[field]);
  if (thresholds && typeof thresholds === "object") values.push(...Object.values(thresholds));
  if (values.some(invalidThreshold)) return {
    code: "invalid_params", message: "Compaction thresholds must be positive whole numbers, or null to inherit.",
  };
  if (!supported && needsCompactionSupport(method, params)) return {
    code: "unknown_method", message: "Update the bridge to set compaction defaults by harness.",
  };
  return null;
}
