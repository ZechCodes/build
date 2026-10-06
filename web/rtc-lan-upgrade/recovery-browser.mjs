import { connectionDiagnosticHistory } from "/src/core/connectionDiagnostics.js";

const waitFor = async (predicate, milliseconds, description) => {
  const deadline = Date.now() + milliseconds;
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error(`deadline: ${description}`);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
};
const isDirect = (state) => state.selected?.state === "succeeded" && state.selected.nominated
  && state.selected.localType !== "relay" && state.selected.remoteType !== "relay";

// Counted consent loss in the disposable phone namespace triggers native
// recovery. The real peer link owns failure detection and every restart.
export async function observeRecovery({ rpc, signaling, fixture, link, stats, pull, applicationCall,
  hostCandidate, appRpcPaths }) {
  const before = await stats();
  if (!isDirect(before) || before.restarts !== 0) throw new Error("recovery must begin on the original direct pair");
  await applicationCall("session.hello");
  const firstPull = await pull();
  await window.fixtureRelease({ before, firstPull, hostCandidate });
  const snapshots = [];
  window.recoverySnapshots = snapshots;
  const nativeStats = async () => {
    const state = await stats();
    snapshots.push({ at: Date.now(), ...state });
    return state;
  };
  const failedAt = Date.now();
  await window.fixtureBlockDirect();
  await waitFor(async () => {
    const state = await nativeStats();
    return state.restarts === 1 && !link.recovery.snapshot().recovering && link.transportPath() === "turn"
      && state.selected?.state === "succeeded" && state.selected.nominated;
  }, 35000, "native direct loss triggers automatic recovery on TURN");
  const turnObservedAt = Date.now();
  const reconnectAt = connectionDiagnosticHistory().findLast((row) => row.event === "connected" && row.phase === "restart").at;
  const recovered = await stats();
  const turnPull = await pull();
  await window.fixtureRecoveryState({ before, recovered, failedAt, reconnectAt, firstPull, turnPull,
    turnObservedAt, diagnostics: connectionDiagnosticHistory() });
  if (recovered.connections !== 1 || recovered.localUfrag === before.localUfrag || recovered.remoteUfrag === before.remoteUfrag) {
    throw new Error("the actual recovery must change both ICE credentials on one peer");
  }
  // Keep native success responses gated until the recovery checklist has run
  // unanswered. Authenticated browser requests can already produce bridge
  // PRFLX evidence, matching the phone export while TURN carries the pull.
  await waitFor(() => Date.now() - reconnectAt >= 16000, 17000, "native recovery checks run unanswered before release");
  await window.fixtureReleaseDirect();
  await waitFor(async () => {
    const state = await nativeStats();
    return state.restarts === 2 && isDirect(state) && link.transportPath() === "direct"
      && !link.recovery.snapshot().recovering;
  }, 20000, "the production optional restart nominates fresh direct after recovery TURN");
  const directObservedAt = Date.now();
  const directBefore = await stats();
  const directPull = await pull();
  await waitFor(async () => {
    const current = await nativeStats();
    return current.selected?.id === directBefore.selected.id
      && current.selected.bytesSent > directBefore.selected.bytesSent
      && current.selected.bytesReceived > directBefore.selected.bytesReceived;
  }, 1500, "the selected native direct pair accounts for the encrypted full pull");
  const after = await stats();
  if (after.connections !== 1 || after.localUfrag === recovered.localUfrag || after.remoteUfrag === recovered.remoteUfrag) {
    throw new Error("optional upgrade must reuse one peer with fresh native credentials");
  }
  const diagnostics = connectionDiagnosticHistory();
  const carryingDirectAt = diagnostics.findLast((row) => row.event === "carrying" && row.path === "direct").at;
  const result = { mode: "recovery-unresolved", before, recovered, after, firstPull, turnPull, directPull,
    hostCandidate, appRpcPaths, failedAt, reconnectAt, turnObservedAt, directObservedAt,
    snapshots, carryingDirectAt, reconnectToDirectMs: carryingDirectAt - reconnectAt, diagnostics };
  await rpc.call("rtc.close", {}, { carrier: signaling, timeoutMs: 10000 });
  link.close("native recovery regression finished");
  fixture.close();
  return result;
}
