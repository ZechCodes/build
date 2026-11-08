// Creation progress is display evidence. It has no authority over an outcome.
const creationMethods = new Set(["environment.create", "environment.create_project", "workspace.create"]);
const progressStates = new Set(["running", "done", "failed"]);

export const isCreationWrite = method => creationMethods.has(method);
export const creationTransportIntent = method => isCreationWrite(method) ? { transportAttempted: false } : {};
export const nextCreationEvidence = row => (row.creationEvidence || 0) + 1;

export function creationSendEvidenceMatches(retained, sent) {
  return retained.journalIncarnation === sent.journalIncarnation && retained.creationSendToken === sent.creationSendToken
    && retained.revision === sent.revision && retained.state === sent.state && retained.acknowledged === sent.acknowledged
    && (retained.creationEvidence || 0) === (sent.creationEvidence || 0);
}
export function creationWritesSupported(greeting, method) {
  if (!isCreationWrite(method)) return true;
  return greeting?.capabilities?.includes(method === "environment.create" ? "environments.create" : "environments") === true;
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
