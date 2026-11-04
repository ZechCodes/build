// Closed labels only: scope identities, filters and opaque tokens never enter
// the diagnostic recorder.
const methods = new Set(["sync.head", "sync.snapshot", "sync.changes"]);
const kinds = new Set(["project_tasks", "task"]);
export const scopedSyncDiagnosticKind = (method, params) => methods.has(method) && kinds.has(params?.scope?.kind)
  ? params.scope.kind : "rpc";
