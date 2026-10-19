// Shared vocabulary for configured intent and observations of an opened session.
export const COMPACTION_TIMING = "Applies when the agent next starts or resumes. Running work keeps its current setting.";
export const COMPACTION_SAVED = "Saved. Applies on the next start or resume.";
export const COMPACTION_FALLBACK = "This harness version checks the threshold between turns. A long turn can exceed it.";

export const positiveCompactionTokens = (value) => Number.isSafeInteger(value) && value > 0;

/** Abbreviate only exact thousands. Preserve unsupported wire decimals verbatim. */
export function compactionTokenLabel(policy) {
  if (policy?.tokens_decimal) return `${policy.tokens_decimal} tokens`;
  const tokens = policy?.tokens;
  if (!positiveCompactionTokens(tokens)) return "Unknown";
  return `${tokens % 1000 === 0 ? `${tokens / 1000}k` : tokens.toLocaleString("en-US")} tokens`;
}

export function inheritedCompactionLabel(policy) {
  if (!policy) return "Use inherited setting";
  const source = { workspace: "workspace", project: "project", device: "device", default: "default" }[policy.source?.kind];
  return `Use inherited setting — ${compactionTokenLabel(policy)}${source ? ` from ${source}` : ""}`;
}

export function compactionPendingLabel(compaction) {
  if (!compaction?.pending_change) return "";
  return `Configured: ${compactionTokenLabel(compaction.configured)} · Applied at last start: ${compactionTokenLabel(compaction.active)}`;
}

export function compactionObservationCopy(compaction) {
  const applied = compaction?.active ? `Applied at last start: ${compactionTokenLabel(compaction.active)}.` : "Applied setting unknown.";
  const fallback = compaction?.enforcement === "between_turn" ? COMPACTION_FALLBACK : "";
  return [compactionPendingLabel(compaction) || applied, fallback, COMPACTION_TIMING].filter(Boolean).join(" ");
}
