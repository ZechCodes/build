// Creation requirements and progress evidence; only terminal records establish outcomes.
// Cached offers and live admission use the same per-method requirement.
const creationMethods = new Map([
  ["environment.create", {kind:"new-environment", flag:"create", capability:"environments.create"}],
  ["environment.create_project", {kind:"environment", flag:"createProject", capability:"environments", recoveryAction:"resume"}],
  ["workspace.create", {kind:"project", flag:"createProject", capability:"environments", recoveryAction:"retry"}],
  ["environment.discard_unused", {flag:"create", capability:"environments.create", recoveryAction:"discardEnvironment"}],
  ["project.discard_unused", {flag:"createProject", capability:"environments", recoveryAction:"discard"}],
]);
const progressStates = new Set(["running", "done", "failed"]);

export const isCreationWrite = method => Boolean(creationMethods.get(method)?.kind);
export const creationTransportIntent = method => isCreationWrite(method) ? { transportAttempted: false } : {};
export const nextCreationEvidence = row => (row.creationEvidence || 0) + 1;

export function creationSendEvidenceMatches(retained, sent) {
  return retained.journalIncarnation === sent.journalIncarnation && retained.creationSendToken === sent.creationSendToken
    && retained.revision === sent.revision && retained.state === sent.state && retained.acknowledged === sent.acknowledged
    && (retained.creationEvidence || 0) === (sent.creationEvidence || 0);
}
export function creationWritesSupported(greeting, method) {
  const requirement = creationMethods.get(method);
  return !requirement || greeting?.capabilities?.includes(requirement.capability) === true;
}

export function creationSupportForMethod(support, method) {
  const requirement = creationMethods.get(method);
  return Boolean(requirement && support?.[requirement.flag] === true);
}
export const environmentWizardSupported = support => creationSupportForMethod(support,"environment.create_project") || creationSupportForMethod(support,"environment.create");
export function creationMethodForTarget(target) {
  for (const [method, requirement] of creationMethods) {
    if (requirement.kind && requirement.kind === target.kind) return method;
  }
  return null;
}
export function creationMethodForRecovery(action, originalMethod) {
  if (action === "back") return isCreationWrite(originalMethod) ? originalMethod : null;
  for (const [method, requirement] of creationMethods) {
    if (requirement.recoveryAction === action) return method;
  }
  return null;
}
// Admission diagnostics are retained as a bounded routing hint, never arbitrary
// bridge detail payloads. Terminal resource evidence keeps its existing path.
export function creationRefusalDetails(details) {
  const creation = details?.creation || {};
  const retained = {};
  if (["sources","environment","project","workspace"].includes(creation.step)) retained.step = creation.step;
  if (["environment","project"].includes(creation.source_scope)) retained.source_scope = creation.source_scope;
  return Object.keys(retained).length ? {creation:retained} : undefined;
}

export const creationNeverSent = row => row.transportAttempted === false && row.state === "queued"
  && !row.acknowledged && !row.progress?.length;

export function unsupportedCreationOutcome(row) {
  const error = { code: "write_unsupported", message: "This device needs an update before it can create this workspace. Nothing was created.", retryable: false };
  const confirmation = { type: "writes.confirmed", operation_id: row.operationId, state: "failed", effect: "unapplied", revision: row.revision + 1, error };
  return { ...row, state: "failed", effect: "unapplied", acknowledged: true, overlayActive: false,
    unresolved: false, checking: false, error, confirmation, revision: confirmation.revision };
}

const validStep = row => typeof row?.step === "string" && row.step.length > 0
  && progressStates.has(row.state) && (row.detail === undefined || typeof row.detail === "string");

export function normalizeWriteProgress(progress) {
  if (!Array.isArray(progress) || !progress.every(validStep)) return null;
  return progress.map(({ step, state, detail }) => ({ step, state, ...(detail === undefined ? {} : { detail }) }));
}
