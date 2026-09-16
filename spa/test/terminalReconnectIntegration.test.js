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

const { followTerminalDevice, provideTerminalSessions, subscribeTerminalStatus, terminalManager } =
  await import("../src/terminal/manager.js");

function answeringCarrier(name, { answerImmediately = true } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const carrier = {
    name,
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
      if (answerImmediately) queueMicrotask(() => carrier.answer(envelope));
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
  afterAll(() => terminalManager().close());

  beforeEach(() => {
    contexts.clear();
    events.length = 0;
    sessions.length = 0;
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
});
