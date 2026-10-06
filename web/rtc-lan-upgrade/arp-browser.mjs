// Findings-only observation; uses the production link and its restart budget.
export async function observeArp({ mode, rpc, signaling, fixture, link, hostCandidate, stats, pull, applicationCall }) {
  const before = await stats();
  await applicationCall("session.hello");
  const firstPull = await pull();
  const snapshots = [];
  const deadline = hostCandidate.gatheredAt + 26000;
  while (true) {
    const state = await stats();
    snapshots.push({ at: Date.now(), ...state });
    const direct = state.selected?.state === "succeeded" && state.selected.nominated
      && state.selected.localType !== "relay" && state.selected.remoteType !== "relay"
      && link.transportPath() === "direct" && !link.recovery.snapshot().recovering;
    if (direct || Date.now() >= deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  const finalPull = await pull();
  const after = await stats();
  await rpc.call("rtc.close", {}, { carrier: signaling, timeoutMs: 10000 });
  link.close("passive ARP observation finished");
  fixture.close();
  return { mode, before, after, hostCandidate, firstPull, finalPull, snapshots };
}
