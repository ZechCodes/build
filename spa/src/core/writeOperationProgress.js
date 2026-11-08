// Creation progress is display evidence. It has no authority over an outcome.
const creationMethods = new Set(["environment.create_project", "workspace.create"]);
const progressStates = new Set(["running", "done", "failed"]);

export const isCreationWrite = method => creationMethods.has(method);
export const creationWritesSupported = (greeting, method) => !isCreationWrite(method)
  || greeting?.capabilities?.includes("environments") === true;

const validStep = row => typeof row?.step === "string" && row.step.length > 0
  && progressStates.has(row.state) && (row.detail === undefined || typeof row.detail === "string");

export function normalizeWriteProgress(progress) {
  if (!Array.isArray(progress) || !progress.every(validStep)) return null;
  return progress.map(({ step, state, detail }) => ({ step, state, ...(detail === undefined ? {} : { detail }) }));
}
