// The replacement path as one seam: the real manager mints a new terminal
// session, the real TerminalSocket proves it over the replacement term channel,
// and only then may the rendezvous lease be released.  The app and terminal
// channels share a peer connection in production, so a terminal probe failure
// must never close that carrier from inside the terminal stack.

import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";

const contexts = vi.hoisted(() => new Map());
const events = vi.hoisted(() => []);
const sessions = vi.hoisted(() => []);

vi.mock("@build/secure-transport", () => ({
  ready: async () => {},
  encryptFrame: async ({ sessionKeyB64, outerFields, frameFields }) => ({
    key: sessionKeyB64,
    outerFields,
    frameFields,
  }),
  decryptEnvelope: async ({ sessionKeyB64, envelope }) => {
    if (envelope.key !== sessionKeyB64) throw new Error("wrong terminal session");
    return { payload: envelope.frameFields.payload };
  },
}));

vi.mock("../src/appState.js", async () => ({ App: (await import("../src/app.js")).App }));
vi.mock("../src/app.js", () => ({
  App: {
    route: { name: "branch", deviceId: "dev-a", projectId: "project-a" },
    devices: [{ id: "dev-a", status: "online" }],
    selectedDeviceId: "dev-a",
  },
}));

vi.mock("../src/core/deviceContexts.js", async () => ({
  ...(await vi.importActual("../src/core/deviceContexts.js")),
  contextFor: (deviceId) => contexts.get(deviceId) || null,
}));

const { followTerminalDevice, provideTerminalSessions, resetTerminalManager, subscribeTerminalStatus, terminalManager } =
  await import("../src/terminal/manager.js");
const { App } = await import("../src/app.js");
const { connectionDiagnosticHistory } = await import("../src/core/connectionDiagnostics.js");

function answeringCarrier(name, { answerImmediately = true } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const carrier = {
    name,
    // Mutable, so a case can let the handshake through and then have the
    // bridge go quiet — which is the state a loaded daemon is in.
    answerImmediately,
    sent: [],
    close: vi.fn(() => closeListeners.forEach((listener) => listener())),
    onEnvelope(listener) {
      envelopeListeners.add(listener);
      return () => envelopeListeners.delete(listener);
    },
    onClose(listener) {
      closeListeners.add(listener);
      return () => closeListeners.delete(listener);
    },
    async send(envelope) {
      carrier.sent.push(envelope);
      const { id, method } = envelope.frameFields.payload;
      events.push(`${name}:${method}`);
      if (carrier.answerImmediately) queueMicrotask(() => carrier.answer(envelope));
    },
    answer(envelope = carrier.sent.at(-1)) {
      const { id, method } = envelope.frameFields.payload;
      const reply = { id, ok: true, result: method === "ping" ? { pong: true } : {} };
      for (const listener of envelopeListeners) {
        listener({ key: envelope.key, frameFields: { payload: reply } });
      }
    },
  };
  return carrier;
}

const settle = async () => {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
};

describe("terminal session handoff after a bridge restart", () => {
  afterAll(() => resetTerminalManager());

  beforeEach(() => {
    resetTerminalManager();
    contexts.clear();
    events.length = 0;
    sessions.length = 0;
    App.route = { name: "branch", deviceId: "dev-a", projectId: "project-a" };
  });

  it("remints, proves the replacement session before releasing its lease, and stays live past five seconds", async () => {
    vi.useFakeTimers();
    try {
      provideTerminalSessions(async (deviceId) => {
        const number = sessions.length + 1;
        const lease = {
          sessionId: `terminal-${number}`,
          sessionKeyB64: `key-${number}`,
          deviceId,
          release: vi.fn(() => events.push(`lease-${number}:release`)),
        };
        sessions.push(lease);
        return lease;
      });

      const firstTerm = answeringCarrier("first");
      const sharedAppCarrier = { close: vi.fn() };
      contexts.set("dev-a", {
        deviceId: "dev-a",
        call: async () => ({}),
        peerLink: { app: sharedAppCarrier, term: firstTerm },
      });

      const socket = terminalManager();
      await settle();
      await vi.advanceTimersByTimeAsync(0);
      expect(socket.deviceId).toBe("dev-a");
      expect(events.slice(0, 2)).toEqual(["first:ping", "lease-1:release"]);

      // The daemon restarted: its old terminal session is gone and the device
      // context now owns a new peer link, though it is still the same device.
      const replacementTerm = answeringCarrier("replacement");
      contexts.set("dev-a", {
        deviceId: "dev-a",
        call: async () => ({}),
        peerLink: { app: sharedAppCarrier, term: replacementTerm },
      });
      followTerminalDevice({ freshSession: true });
      await settle();
      await vi.advanceTimersByTimeAsync(0);

      expect(sessions).toHaveLength(2);
      expect(socket.deviceId).toBe("dev-a");
      expect(events.slice(2, 4)).toEqual(["replacement:ping", "lease-2:release"]);

      // Silence causes the real liveness watcher to probe at two seconds. The
      // replacement bridge answers, so neither half of the shared peer closes.
      await vi.advanceTimersByTimeAsync(6000);
      expect(replacementTerm.sent.filter((frame) => frame.frameFields.payload.method === "ping").length).toBeGreaterThan(1);
      expect(replacementTerm.close).not.toHaveBeenCalled();
      expect(sharedAppCarrier.close).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  /** The terminal stack as it stands after one session has proved itself: the
   *  socket live on `term`, with the app channel beside it on the same peer. */
  async function livePeer({ answerImmediately = true, peerFrameAt = () => 0, peerIsConnected = () => false } = {}) {
    provideTerminalSessions(async (deviceId) => {
      const number = sessions.length + 1;
      const lease = {
        sessionId: `terminal-${number}`,
        sessionKeyB64: `key-${number}`,
        deviceId,
        release: vi.fn(),
      };
      sessions.push(lease);
      return lease;
    });
    const term = answeringCarrier("term", { answerImmediately: true });
    term.peerFrameAt = peerFrameAt;
    term.peerIsConnected = peerIsConnected;
    const app = { close: vi.fn() };
    contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { app, term } });
    const socket = terminalManager();
    await settle();
    await vi.advanceTimersByTimeAsync(0);
    // Past the handshake the bridge stops answering, which is the state under
    // test: a ping that will time out.
    term.answerImmediately = answerImmediately;
    return { socket, term, app };
  }

  // The bug this file's header names, seen end to end: under a load of agents
  // the bridge's pong queued behind other work, the probe's three seconds ran
  // out on a path that was plainly carrying, and the channel the probe closed
  // took the whole connection with it — every few seconds, all day.
  //
  // The peer's two channels are one path, so the app session's frames are
  // proof for the terminal session too: a quiet terminal beside a busy app is
  // not a probe's business at all.
  it("asks nothing of a path the peer is carrying on", async () => {
    vi.useFakeTimers();
    try {
      const { term, app } = await livePeer({ answerImmediately: false, peerFrameAt: () => Date.now() });

      await vi.advanceTimersByTimeAsync(6000);

      const pings = term.sent.filter((frame) => frame.frameFields.payload.method === "ping");
      expect(pings, "the handshake's ping and no other").toHaveLength(1);
      expect(term.close, "the wire the peer is carrying on is not closed").not.toHaveBeenCalled();
      expect(app.close, "and the app session is not touched").not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  // Silence at probe time, and the peer carrying again while the ping is out:
  // the path is up and what did not answer is this SESSION. The terminals take
  // a fresh one on the wire that is still carrying; nothing else moves.
  it("re-establishes the terminals when the peer carries while the ping is out", async () => {
    vi.useFakeTimers();
    try {
      let carryingSince = 0;
      const { term, app } = await livePeer({
        answerImmediately: false,
        peerFrameAt: () => carryingSince,
      });
      const before = sessions.length;
      // The app session hears something the moment the probe's ping goes out.
      const wasSent = term.send;
      term.send = async (envelope) => {
        if (envelope.frameFields.payload.method === "ping") carryingSince = Date.now();
        return wasSent(envelope);
      };

      // The proof window holds the first ping back to four seconds, and the
      // ping itself waits three for a pong.
      await vi.advanceTimersByTimeAsync(9000);

      expect(term.close, "a carrying path keeps its wire").not.toHaveBeenCalled();
      expect(app.close).not.toHaveBeenCalled();
      expect(sessions.length, "the terminals take a fresh session instead").toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
    }
  });

  // The churn as the maintainer saw it, and as the compose stack reproduces it:
  // a bridge that does not answer in three seconds over a path the browser's
  // own ICE is still holding open. An application ping measures the path AND
  // the daemon behind it; ICE measures the path, with consent checks of its
  // own. So ICE's word outranks the silence, and the peer stands.
  it("keeps a peer ICE is still holding, however long the bridge takes to answer", async () => {
    vi.useFakeTimers();
    try {
      const { term, app } = await livePeer({
        answerImmediately: false,
        peerFrameAt: () => 0,
        peerIsConnected: () => true,
      });
      const before = sessions.length;

      await vi.advanceTimersByTimeAsync(9000);

      expect(term.close, "the path ICE holds is not closed").not.toHaveBeenCalled();
      expect(app.close).not.toHaveBeenCalled();
      expect(sessions.length, "the terminals re-establish instead").toBeGreaterThan(before);
      const judged = connectionDiagnosticHistory()
        .filter((record) => record.event === "terminal-session" && record.state === "liveness-timeout");
      expect(judged.at(-1).vouched).toBe("ice-connected");
      expect(judged.at(-1).channel).toBe("term");
    } finally {
      vi.useRealTimers();
    }
  });

  // The other half, which must keep working: a path nothing vouches for — no
  // frames, and ICE not holding it either — still goes.
  it("closes the wire when nothing is carrying anywhere, naming the peer", async () => {
    vi.useFakeTimers();
    try {
      const { term } = await livePeer({ answerImmediately: false, peerFrameAt: () => 0 });

      await vi.advanceTimersByTimeAsync(9000);

      expect(term.close).toHaveBeenCalledWith("liveness-timeout");
      const judged = connectionDiagnosticHistory()
        .filter((record) => record.event === "terminal-session" && record.state === "liveness-timeout");
      expect(judged.at(-1).channel, "the record says which channel was judged").toBe("peer");
      expect(judged.at(-1).vouched, "and that nothing vouched for the path").toBe("nothing");
    } finally {
      vi.useRealTimers();
    }
  });

  it("lets a newer peer win while the old session acknowledgement is pending", async () => {
    vi.useFakeTimers();
    const statuses = [];
    const unsubscribe = subscribeTerminalStatus((status) => statuses.push(status));
    try {
      provideTerminalSessions(async (deviceId) => {
        const number = sessions.length + 1;
        const lease = {
          sessionId: `overlap-${number}`,
          sessionKeyB64: `overlap-key-${number}`,
          deviceId,
          release: vi.fn(() => events.push(`overlap-${number}:release`)),
        };
        sessions.push(lease);
        return lease;
      });

      const oldTerm = answeringCarrier("old-pending", { answerImmediately: false });
      const app = { close: vi.fn() };
      contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { app, term: oldTerm } });
      const socket = terminalManager();
      if (socket.deviceId) followTerminalDevice({ freshSession: true });
      await settle();
      expect(events).toContain("old-pending:ping");
      expect(events).not.toContain("overlap-1:release");

      const newTerm = answeringCarrier("new-live");
      contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { app, term: newTerm } });
      followTerminalDevice({ freshSession: true });
      await settle();
      await vi.advanceTimersByTimeAsync(0);

      expect(socket.deviceId).toBe("dev-a");
      expect(events).toContain("new-live:ping");
      expect(events).toContain("overlap-1:release");
      expect(events).toContain("overlap-2:release");
      const statusesBeforeLateAck = [...statuses];

      // A reply from the retired bridge/session is now meaningless. It cannot
      // repaint connection state or disturb the replacement carrier.
      oldTerm.answer();
      await settle();
      await vi.advanceTimersByTimeAsync(6000);
      expect(statuses).toEqual(statusesBeforeLateAck);
      expect(oldTerm.close).not.toHaveBeenCalled();
      expect(newTerm.close).not.toHaveBeenCalled();
      expect(app.close).not.toHaveBeenCalled();
    } finally {
      unsubscribe();
      vi.useRealTimers();
    }
  });

  it("releases a terminal lease after an unacknowledged initial ping without closing the shared peer", async () => {
    vi.useFakeTimers();
    try {
      provideTerminalSessions(async (deviceId) => {
        const lease = {
          sessionId: "unacknowledged",
          sessionKeyB64: "unacknowledged-key",
          deviceId,
          release: vi.fn(() => events.push("unacknowledged:release")),
        };
        sessions.push(lease);
        return lease;
      });
      const silentTerm = answeringCarrier("silent", { answerImmediately: false });
      const app = { close: vi.fn() };
      contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { app, term: silentTerm } });

      const socket = terminalManager();
      if (socket.deviceId) followTerminalDevice({ freshSession: true });
      await settle();
      await vi.advanceTimersByTimeAsync(3100);
      await settle();

      expect(events).toContain("silent:ping");
      expect(events).toContain("unacknowledged:release");
      expect(silentTerm.close).not.toHaveBeenCalled();
      expect(app.close).not.toHaveBeenCalled();

      // No liveness watcher starts for a session that never acknowledged, so
      // another full liveness window cannot subsequently kill the peer.
      await vi.advanceTimersByTimeAsync(6000);
      expect(silentTerm.close).not.toHaveBeenCalled();
      expect(app.close).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("remints the original device when a route returns while another device is confirming", async () => {
    const slowB = answeringCarrier("slow-b", { answerImmediately: false });
    const carrierA = answeringCarrier("carrier-a");
    contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { term: carrierA } });
    contexts.set("dev-b", { deviceId: "dev-b", call: async () => ({}), peerLink: { term: slowB } });
    provideTerminalSessions(async (deviceId) => {
      const number = sessions.length + 1;
      const lease = {
        sessionId: `${deviceId}-${number}`,
        sessionKeyB64: `${deviceId}-key-${number}`,
        deviceId,
        release: vi.fn(),
      };
      sessions.push(lease);
      return lease;
    });

    terminalManager();
    await settle();
    App.route = { name: "branch", deviceId: "dev-b", projectId: "project-b" };
    followTerminalDevice();
    await settle();
    expect(events).toContain("slow-b:ping");

    App.route = { name: "branch", deviceId: "dev-a", projectId: "project-a" };
    followTerminalDevice();
    await settle();

    expect(sessions.map((session) => session.deviceId)).toEqual(["dev-a", "dev-b", "dev-a"]);
    expect(events.filter((event) => event === "carrier-a:ping")).toHaveLength(2);
  });

  it("drops an old-account mint after reset and reconnects the same device id with a fresh socket", async () => {
    let resolveOld;
    const oldMint = new Promise((resolve) => { resolveOld = resolve; });
    const oldLease = {
      sessionId: "old-account",
      sessionKeyB64: "old-key",
      deviceId: "dev-a",
      release: vi.fn(),
    };
    provideTerminalSessions(() => oldMint);
    contexts.set("dev-a", {
      deviceId: "dev-a",
      call: async () => ({}),
      peerLink: { term: answeringCarrier("old-account") },
    });
    terminalManager();
    await settle();

    resetTerminalManager();
    const inheritedStatus = vi.fn();
    const unsubscribe = subscribeTerminalStatus(inheritedStatus);
    expect(inheritedStatus).not.toHaveBeenCalled();
    unsubscribe();
    const replacement = answeringCarrier("replacement-account");
    contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { term: replacement } });
    provideTerminalSessions(async () => ({
      sessionId: "new-account",
      sessionKeyB64: "new-key",
      deviceId: "dev-a",
      release: vi.fn(),
    }));
    const freshSocket = terminalManager();
    await settle();
    resolveOld(oldLease);
    await settle();

    expect(freshSocket.deviceId).toBe("dev-a");
    expect(events).toContain("replacement-account:ping");
    expect(events).not.toContain("old-account:ping");
    expect(oldLease.release).toHaveBeenCalledOnce();
  });

  it("does not resurrect a carrier when a connecting status synchronously moves to another device", async () => {
    const carrierA = answeringCarrier("reentrant-a");
    const carrierB = answeringCarrier("reentrant-b");
    contexts.set("dev-a", { deviceId: "dev-a", call: async () => ({}), peerLink: { term: carrierA } });
    contexts.set("dev-b", { deviceId: "dev-b", call: async () => ({}), peerLink: { term: carrierB } });
    let resolveB;
    const waitingB = new Promise((resolve) => { resolveB = resolve; });
    provideTerminalSessions((deviceId) => deviceId === "dev-a"
      ? Promise.resolve({ sessionId: "reentrant-a", sessionKeyB64: "key-a", deviceId, release: vi.fn() })
      : waitingB);

    const statuses = [];
    let moved = false;
    const unsubscribe = subscribeTerminalStatus((status) => {
      statuses.push(status);
      if (status !== "connecting" || moved) return;
      moved = true;
      App.route = { name: "branch", deviceId: "dev-b", projectId: "project-b" };
      followTerminalDevice();
    });
    terminalManager();
    await settle();

    expect(events).not.toContain("reentrant-a:ping");
    expect(statuses).not.toContain("connected");

    resolveB({ sessionId: "reentrant-b", sessionKeyB64: "key-b", deviceId: "dev-b", release: vi.fn() });
    await settle();
    expect(events).toContain("reentrant-b:ping");
    unsubscribe();
  });
});
