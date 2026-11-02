// Terminal RTC ownership is independent of an HTTP application session. This
// registry never changes a device context or closes the HTTP session it follows.
const owners = new Map();
const retired = new WeakSet();
let configuration = { open: async () => { throw new Error("terminal RTC is unavailable"); }, isCurrent: () => false, onChanged: () => {} };
const quietly = (work) => { try { work(); } catch { /* finish disposing the owner */ } };

export function configureTerminalPeerLinks({ open, isCurrent, onChanged = () => {} }) {
  clearTerminalPeerLinks();
  configuration = { open, isCurrent, onChanged };
}

export function terminalPeerLink(context) {
  if (!context) return null;
  const session = context.session;
  if (session?.transport !== "http") return context.peerLink || null;
  if (retired.has(session)) return null;
  let owner = owners.get(session);
  if (!owner) {
    owner = createLazyOwner(context, session, configuration);
    owners.set(session, owner);
    owner.watchSession();
  }
  return retired.has(session) ? null : owner;
}

export function releaseTerminalPeerLink(session) {
  if (!session || typeof session !== "object") return;
  retired.add(session);
  const owner = owners.get(session);
  owners.delete(session);
  owner?.close();
}

export function clearTerminalPeerLinks() {
  for (const session of [...owners.keys()]) releaseTerminalPeerLink(session);
}

function createLazyOwner(context, session, config) {
  let closed = false;
  let helper = null;
  let opening = null;
  let abort = null;
  let stopHelper = () => {};
  let stopSession = () => {};
  let rejectClosed;
  const stopped = new Promise((_, reject) => { rejectClosed = reject; });
  stopped.catch(() => {});
  const current = () => !closed && context.session === session && config.isCurrent(context);
  const changed = () => { if (current()) quietly(() => config.onChanged(context)); };
  const close = () => {
    if (closed) return;
    closed = true;
    rejectClosed(new Error("terminal RTC owner retired"));
    abort?.abort();
    quietly(stopSession); quietly(stopHelper);
    quietly(() => helper?.close());
    helper = null;
  };
  const assertCurrent = () => {
    if (current()) return;
    close();
    throw new Error("terminal RTC owner retired");
  };
  const guard = (pending) => Promise.race([pending, stopped]);
  const lost = (owned) => {
    if (helper !== owned) return;
    stopHelper(); stopHelper = () => {};
    helper = null;
    changed();
  };
  const acceptHelper = (owned) => {
    if (!current()) { quietly(() => owned.close()); assertCurrent(); }
    helper = owned;
    const unsubscribe = owned.onClose(() => lost(owned));
    if (helper === owned) stopHelper = unsubscribe;
    else unsubscribe();
    changed();
    if (helper !== owned) throw new Error("terminal RTC parent closed during setup");
    return owned;
  };
  const ensureHelper = () => {
    assertCurrent();
    if (helper) return Promise.resolve(helper);
    if (!opening) {
      const attempt = new AbortController();
      abort = attempt;
      opening = Promise.resolve().then(() => {
        assertCurrent();
        return config.open({ deviceId: context.deviceId, signal: attempt.signal });
      }).then(acceptHelper).finally(() => { opening = null; if (abort === attempt) abort = null; });
    }
    return guard(opening);
  };
  const assertHelper = (owned) => {
    assertCurrent();
    if (helper !== owned) throw new Error("terminal RTC parent replaced");
  };
  const bind = async (owned, minted) => {
    try {
      assertHelper(owned);
      const carrier = await guard(owned.bindTerminal(minted));
      assertHelper(owned);
      changed();
      return carrier;
    } catch (error) { quietly(() => minted.release?.()); throw error; }
  };
  const owner = {
    get term() { return helper?.term || null; },
    get recovery() { return helper?.recovery || null; },
    close,
    watchSession() {
      const unsubscribe = session.onClose?.(() => releaseTerminalPeerLink(session));
      if (closed) unsubscribe?.();
      else stopSession = unsubscribe || (() => {});
    },
    async mintTerminal() {
      const owned = await ensureHelper();
      const pending = Promise.resolve(owned.mintTerminal()).then((minted) => {
        try { assertHelper(owned); }
        catch (error) { quietly(() => minted.release?.()); throw error; }
        return minted;
      });
      const minted = await guard(pending);
      const carrier = await bind(owned, minted);
      return { ...minted, carrier, peerLink: owner, bridgeEpoch: owned.bridgeEpoch };
    },
    async bindTerminal(minted) { return bind(await ensureHelper(), minted); },
  };
  return owner;
}
