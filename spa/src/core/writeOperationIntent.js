// Client intent is persisted exactly as sent, independently of transport IDs.
export const WRITE_STATUS_LIMIT = 128;
export const WRITE_RETRY_DELAYS = [1000, 2000, 5000, 10000, 30000];
const TERMINAL = new Set(["succeeded", "failed", "result_expired"]);
const SUMMARY_STATES = new Set(["accepted", "executing", "succeeded", "failed", "not_found", "result_expired"]);
export const knownWriteSummary = summary => SUMMARY_STATES.has(summary?.state);
export const terminalWrite = row => TERMINAL.has(row?.state);
export const needsWriteStatus = row => !terminalWrite(row) && !row?.unresolved;
export const provenExpiredFailure = row => row?.state === "result_expired" && row.outcome === "failed" && row.effect === "unapplied";
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
}
export const copyIntent = value => canonical(JSON.parse(JSON.stringify(value)));

function stringsIn(value, found = new Set()) {
  if (typeof value === "string") found.add(value);
  if (value && typeof value === "object") Object.values(value).forEach(item => stringsIn(item, found));
  return found;
}
function consumesLayer(row, references) {
  if ([...stringsIn(row.reservedIds)].some(id => references.has(id))) return true;
  return (row.overlay || []).some(layer => references.has(layer.address?.entityId));
}
export function writeDependencies(input, rows) {
  const references = stringsIn(input.params);
  const inferred = rows.filter(row => row.acknowledged && !terminalWrite(row) && consumesLayer(row, references)).map(row => row.operationId);
  const required = [...new Set([...inferred, ...(input.uses || [])])];
  const after = [...new Set(input.after === undefined ? required : input.after)];
  if (required.some(id => !after.includes(id))) throw new Error("A required write dependency is missing");
  if (after.length > 8) throw new Error("A write supports at most eight dependencies");
  return input.after === undefined && !after.length ? undefined : after;
}
export function writeSubmissionOrder(left, right) {
  const leftOrder = left.localOrder ?? left.createdAt ?? 0;
  const rightOrder = right.localOrder ?? right.createdAt ?? 0;
  return leftOrder - rightOrder || (left.createdAt || 0) - (right.createdAt || 0) || left.operationId.localeCompare(right.operationId);
}
export function writeRefusal(message, code = "write_unsupported") {
  return Object.assign(new Error(message), { error_code: code, retryable: false });
}
export function wireOperationId(frame) {
  return frame?.write?.operation_id || frame?.operation_id;
}
export function wireRevision(frame) {
  return frame?.write?.revision ?? frame?.revision;
}
export function writeSummaryFrame(summary, epoch) {
  if (summary.confirmation) return { ...summary.confirmation, bridge_epoch: epoch };
  if (["succeeded", "failed"].includes(summary.state)) return { ...summary, bridge_epoch: epoch, type: "writes.confirmed" };
  if (summary.ack) return { ok: true, bridge_epoch: epoch, write: summary.ack, result: summary.result };
  if (summary.write || summary.type) return summary;
  return { ...summary, bridge_epoch: summary.bridge_epoch || epoch, type: "writes.confirmed" };
}
export function operationFailure(frame) {
  const detail = frame.error;
  const message = typeof detail === "string" ? detail : detail?.message;
  return Object.assign(new Error(message || "Your change was not saved"), {
    error_code: detail?.code || frame.error_code,
    retryable: detail?.retryable ?? frame.retryable ?? false,
    effect: frame.effect, uncertain: ["partial", "unknown"].includes(frame.effect), operation_id: wireOperationId(frame),
  });
}

export function expiredOperationFailure(row) {
  return Object.assign(new Error("Your change wasn’t saved. Its detailed result has expired."), {
    error_code: "result_expired", retryable: false, effect: "unapplied", uncertain: false, operation_id: row.operationId,
  });
}
