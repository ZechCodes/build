const LIMIT = 100;
const history = [];
export function recordConnectionDiagnostic(connection, event, detail = {}) {
  history.push({ at: Date.now(), connection, event, ...detail });
  if (history.length > LIMIT) history.splice(0, history.length - LIMIT);
}
export function connectionDiagnosticHistory() { return history.map((entry) => ({ ...entry })); }
export function clearConnectionDiagnosticHistory() { history.length = 0; }
