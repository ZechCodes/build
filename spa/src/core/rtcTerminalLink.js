import { openSession } from "./session.js";
import { openPeerLink } from "./peerLink.js";
import { selectAdapter, SPA_API_RANGE } from "./bridgeApi/index.js";

const quietly = (work) => { try { work(); } catch { /* finish disposing other owned resources */ } };

/** Owns only this attempt. Releasing a captured lifecycle lease cannot close a
 * successor generation, unlike force-closing a shared rendezvous owner. */
function createOwner(signal) {
  let reason = null;
  let stop;
  const stopped = new Promise((_, reject) => { stop = reject; });
  stopped.catch(() => {});
  const resources = new Set();
  const listeners = new Set();
  const assertOpen = () => { if (reason) throw reason; };
  const close = (error = new Error("terminal RTC link closed")) => {
    if (reason) return;
    reason = error instanceof Error ? error : new Error(String(error));
    stop(reason);
    signal?.removeEventListener("abort", aborted);
    for (const dispose of [...resources].reverse()) quietly(dispose);
    resources.clear();
    for (const listener of [...listeners]) quietly(() => listener(reason));
    listeners.clear();
  };
  const aborted = () => close(signal.reason || new DOMException("Terminal connection cancelled", "AbortError"));
  signal?.addEventListener("abort", aborted, { once: true });
  if (signal?.aborted) aborted();
  const addCleanup = (dispose) => {
    if (reason) { quietly(dispose); throw reason; }
    resources.add(dispose);
    return () => resources.delete(dispose);
  };
  return {
    assertOpen, close, addCleanup,
    current: () => reason === null,
    async wait(pending, dispose) {
      const completed = Promise.resolve(pending).then((value) => {
        if (reason) { if (dispose) quietly(() => dispose(value)); throw reason; }
        if (dispose) addCleanup(() => dispose(value));
        return value;
      });
      return Promise.race([completed, stopped]);
    },
    lease(raw) {
      assertOpen();
      if (!raw) throw new Error("terminal rendezvous generation retired");
      let released = false;
      const release = () => {
        if (released) return;
        released = true;
        remove();
        quietly(() => raw.release());
      };
      const remove = addCleanup(release);
      return {
        rendezvous: raw.rendezvous,
        reacquire: () => raw.reacquire(),
        release,
      };
    },
    onClose(listener) {
      if (reason) { listener(reason); return () => {}; }
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

function createSignaling(owner, session, initialLease) {
  let restartLease = null;
  return {
    async onFailed() {
      owner.assertOpen();
      if (restartLease) throw new Error("terminal RTC restart already negotiating");
      const lease = owner.lease(initialLease.reacquire());
      restartLease = lease;
      try { await owner.wait(session.reattachSignaling()); }
      catch (error) {
        lease.release();
        if (restartLease === lease) restartLease = null;
        throw error;
      }
    },
    onConnected(signalingComplete) {
      const lease = restartLease;
      restartLease = null;
      void Promise.resolve(signalingComplete).then(() => lease?.release(), () => lease?.release());
    },
  };
}

function createGreeting(owner, session) {
  let pending = null;
  const greet = () => {
    if (pending) return pending;
    pending = owner.wait(session.rawCall("session.hello", { api_range: SPA_API_RANGE })).then((greeting) => {
      owner.assertOpen();
      const selection = selectAdapter(greeting);
      if (selection.unsupported) throw new Error(`terminal RTC ${selection.unsupported} version unsupported`);
      if (!session.acceptGreeting(greeting, { isCurrent: owner.current })) throw new Error("terminal RTC greeting retired");
      owner.assertOpen();
      session.installAdapter(selection);
    }).finally(() => { pending = null; });
    return pending;
  };
  return greet;
}

function watchParent(owner, session, link, greet) {
  const regreet = () => { if (owner.current()) void greet().catch(owner.close); };
  const restored = () => {
    if (!owner.current()) return;
    session.resetSqlReads();
    regreet();
  };
  owner.addCleanup(link.onRestored(restored));
  owner.addCleanup(session.bridgeEpoch.onUnexpected(regreet));
}

function terminalInterface(owner, session, link, initialLease) {
  const mintedLeases = new WeakMap();
  return {
    bridgeEpoch: session.bridgeEpoch,
    recovery: link.recovery,
    get term() { return link.term; },
    close: owner.close,
    onClose: owner.onClose,
    async mintTerminal() {
      owner.assertOpen();
      const lease = owner.lease(initialLease.reacquire());
      try {
        const minted = await owner.wait(lease.rendezvous.mint({}));
        owner.assertOpen();
        const result = { ...minted, bridgeEpoch: session.bridgeEpoch, release: lease.release };
        mintedLeases.set(result, lease);
        return result;
      } catch (error) { lease.release(); throw error; }
    },
    async bindTerminal(minted) {
      owner.assertOpen();
      if (minted?.deviceId !== session.deviceId) throw new Error("terminal session belongs to another device");
      try { return await owner.wait(link.bindTerminal(minted)); }
      finally { mintedLeases.get(minted)?.release(); }
    },
  };
}

/** Independent parent RTC session for HTTP-selected devices. It does not
 * install a device context, dispatch application pushes or touch caches.
 * acquireRendezvous returns a createRendezvousLifecycle lease; all reacquires
 * stay on that captured generation. A terminal mint holds its own lease until
 * bindTerminal completes, its release is called, or this owner closes.
 * The caller owns this returned link and closes it on deselection/sign-out.
 */
export async function openRtcTerminalLink({
  deviceId, acquireRendezvous, transport, fetchIceServers, clientId, signal,
  RTCPeerConnectionImpl = globalThis.RTCPeerConnection, openTimeoutMs,
  openSessionImpl = openSession, openPeerLinkImpl = openPeerLink,
}) {
  const owner = createOwner(signal);
  try {
    owner.assertOpen();
    const initialLease = owner.lease(acquireRendezvous());
    const session = await owner.wait(openSessionImpl({
      deviceId, rendezvous: initialLease.rendezvous, transport,
      onLost: () => owner.close(new Error("terminal RTC parent lost")),
    }), (owned) => owned.close());
    const link = await owner.wait(openPeerLinkImpl({
      signal: session.rawCall, onPush: session.onPush, binding: session.peerBinding,
      confirmCarried: () => session.confirmCarried(), fetchIceServers, clientId,
      RTCPeerConnectionImpl, openTimeoutMs, diagnosticId: `${deviceId}:${session.sessionId}:terminal`,
      ...createSignaling(owner, session, initialLease),
    }), (owned) => owned.close());
    session.watchRecovery(() => link.recovery.snapshot().recovering);
    await owner.wait(session.peer(link.app));
    const greet = createGreeting(owner, session);
    await greet();
    watchParent(owner, session, link, greet);
    void Promise.resolve(link.whenInitialSignalingComplete()).then(initialLease.release, initialLease.release);
    owner.assertOpen();
    return terminalInterface(owner, session, link, initialLease);
  } catch (error) { owner.close(error); throw error; }
}
