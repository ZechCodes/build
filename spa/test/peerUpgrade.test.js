// @vitest-environment jsdom
// The connect sequence, in the order the spec sets it out (rules 1 to 5): find
// the device over its rendezvous, mint its session there, open the two
// channels, close the rendezvous, and only then greet and go live. A device
// whose connection cannot be made is blocked with a reason, and nothing ever
// falls back to the relay.
//
// The rendezvous and the session are the real ones here, over a fake socket, so
// what this file can say is what actually went down the wire: `session_init`,
// `rtc.*`, and nothing else (rule 1), on a socket that is closed the moment the
// channels carry (rule 4).

import { describe, it, expect, vi, beforeEach } from "vitest";

const peerLink = vi.hoisted(() => ({ open: vi.fn() }));
const api = vi.hoisted(() => ({ fetchIceServers: vi.fn(async () => [{ urls: ["stun:stun.test"] }]) }));
const account = vi.hoisted(() => ({ pinnedKey: vi.fn(async () => "pk") }));
const terminals = vi.hoisted(() => ({ followTerminalDevice: vi.fn(), deviceId: "dev-a", mint: null }));
const greetings = vi.hoisted(() => ({ greet: vi.fn(async () => true) }));
const changed = vi.hoisted(() => []);

vi.mock("../src/core/peerLink.js", () => ({
  openPeerLink: (options) => peerLink.open(options),
}));
vi.mock("../src/api.js", () => ({
  fetchIceServers: (...args) => api.fetchIceServers(...args),
  fetchGatewayToken: async () => "tok-1",
  fetchDevices: async () => [],
}));
// Which device the terminals are on is the manager's own answer; here it is a
// fixed one, so what this file reads is whether a link event on that device
// moved them and a link event on another one left them alone. The mint the
// connection layer hands over is kept, because that is rule 5's seam.
vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice: (...args) => terminals.followTerminalDevice(...args),
  terminalDeviceId: () => terminals.deviceId,
  provideTerminalSessions: (mint) => {
    terminals.mint = mint;
  },
  // No terminal tab mounts in this suite; the socket and its status are still
  // answered, so a surface that asked for one would get a plain no.
  terminalManager: () => null,
  subscribeTerminalStatus: () => () => {},
}));
// A transparent transport: envelopes are plain objects, so a case can read the
// method inside one exactly as the bridge would.
vi.mock("@build/secure-transport", () => ({
  ready: async () => {},
  createSessionInit: async ({ sessionId, deviceId, sessionKeyB64 }) => ({
    sessionKeyB64: sessionKeyB64 || `key-${sessionId}`,
    sessionInit: { session_id: sessionId, device_id: deviceId },
  }),
  openSessionAccept: async () => {},
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
}));
vi.mock("../src/devices.js", () => ({
  paintDevicePicker: () => {},
  pinnedDeviceTransportKey: (...args) => account.pinnedKey(...args),
  refreshDevices: async () => [],
}));
vi.mock("../src/core/composeView.js", () => ({ flushCaptures: async () => {} }));
vi.mock("../src/core/changeEvents.js", () => ({
  dispatchChangeEvent: (...args) => changed.push(args),
  disarmChangeEvents: () => {},
  greetBridge: (...args) => greetings.greet(...args),
}));

const { App, resetApplication } = await import("../src/app.js");
const { contextFor } = await import("../src/core/deviceContexts.js");
const {
  connectDevice,
  goOffline,
  greetLiveBridge,
  openDeviceSessions,
  openDeviceSettingsSession,
  securityStopText,
} = await import("../src/connection.js");

// Every channel the terminals were sent to ride, in order: what the manager
// reads off the context of the device it follows each time it is told to look.
const handedOver = [];

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async () => {
  for (let index = 0; index < 6; index += 1) await tick();
};

/**
 * The relay socket, as the rendezvous drives it.
 *
 * It answers a `session_init` the way the relay-and-bridge pair does, and every
 * signaling envelope with the id of the device the session was routed to — so
 * an upgrade can tell two machines apart over its own wire. Every client
 * message is kept, which is how a case says what did and did not ride the relay.
 */
class FakeWebSocket {
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.sessions = new Map(); // session id → the device it was routed to
    this.listeners = {};
    this.readyState = 1; // OPEN
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => this.emit("open"));
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  send(text) {
    const message = JSON.parse(text);
    this.sent.push(message);
    if (message.type === "session_init") this.takeInit(message);
    if (message.type === "e2ee_envelope") this.answerCall(message);
  }
  takeInit(message) {
    this.sessions.set(message.session_id, message.route_to.replace("device:", ""));
    if (FakeWebSocket.deviceIsSilent) return; // the bridge is not on the relay
    if (FakeWebSocket.relayDrops) {
      this.close();
      return;
    }
    this.serverSend({ type: "session_accept", session_id: message.session_id, envelope: {} });
  }
  answerCall(message) {
    const asked = message.envelope.frameFields.payload;
    const deviceId = this.sessions.get(message.session_id);
    const reply = FakeWebSocket.refuses
      ? { id: asked.id, ok: false, error: { message: "the bridge would not take it" } }
      : { id: asked.id, ok: true, result: { deviceId, sdp: "answer" } };
    this.serverSend({ type: "e2ee_envelope", session_id: message.session_id, envelope: { frameFields: { payload: reply } } });
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3; // CLOSED
    this.emit("close", {});
  }
  emit(type, event = {}) {
    (this.listeners[type] || []).forEach((fn) => fn(event));
  }
  serverSend(object) {
    this.emit("message", { data: JSON.stringify(object) });
  }
}
FakeWebSocket.instances = [];

const sockets = () => FakeWebSocket.instances;
const liveSockets = () => sockets().filter((socket) => socket.readyState !== 3);

/** Every RPC this browser put on a relay socket, by method. */
const overTheRelay = () =>
  sockets().flatMap((socket) =>
    socket.sent
      .filter((message) => message.type === "e2ee_envelope")
      .map((message) => message.envelope.frameFields.payload.method),
  );

/** Every RPC that went down a channel, by method. */
const overTheChannel = (carrier) => carrier.sent.map((envelope) => envelope.frameFields.payload.method);

/** A DataChannel as the connection layer holds it: something to hand over, a
 *  way to say it closed, and an answer to whatever is asked down it. */
function fakeCarrier(name) {
  const closeListeners = new Set();
  const envelopeListeners = new Set();
  const carrier = {
    name,
    sent: [],
    send: async (envelope) => {
      carrier.sent.push(envelope);
      const asked = envelope.frameFields.payload;
      queueMicrotask(() => carrier.deliver({ id: asked.id, ok: true, result: {} }));
    },
    deliver: (payload) => envelopeListeners.forEach((fn) => fn({ frameFields: { payload } })),
    onEnvelope: (fn) => {
      envelopeListeners.add(fn);
      return () => envelopeListeners.delete(fn);
    },
    onClose: (fn) => {
      closeListeners.add(fn);
      return () => closeListeners.delete(fn);
    },
    close: () => closeListeners.forEach((fn) => fn()),
    drop: () => closeListeners.forEach((fn) => fn()),
  };
  return carrier;
}

const fakeLink = () => ({ app: fakeCarrier("app"), term: fakeCarrier("term"), close: vi.fn() });

/** The peer link opens as the real one does: the ICE servers are minted, the
 *  offer goes over the signaling wire, and the caller is told the channels are
 *  open before the link is handed back. */
function linkOpensWith(linkFor) {
  peerLink.open.mockImplementation(async ({ fetchIceServers, signal }) => {
    const iceServers = await fetchIceServers();
    const answer = await signal("rtc.offer", { sdp: "v=0", ice_servers: iceServers });
    return typeof linkFor === "function" ? linkFor(answer.deviceId) : linkFor;
  });
}

/** Boot the account: every device online, opened at once — what the gate does.
 *  The first device the list names is the one the account calls home. */
async function connect(...deviceIds) {
  App.devices = deviceIds.map((id) => ({ id, name: id === "dev-a" ? "Laptop" : "Studio", status: "online" }));
  await openDeviceSessions().settled;
  await settle();
  return contextFor(deviceIds[0]);
}

beforeEach(() => {
  resetApplication();
  document.body.className = "";
  changed.length = 0;
  FakeWebSocket.instances.length = 0;
  FakeWebSocket.deviceIsSilent = false;
  FakeWebSocket.relayDrops = false;
  FakeWebSocket.refuses = false;
  App.devices = [];
  App.selectedDeviceId = null;
  globalThis.RTCPeerConnection = class {};
  globalThis.WebSocket = FakeWebSocket;
  terminals.deviceId = "dev-a";
  handedOver.length = 0;
  for (const spy of [peerLink.open, api.fetchIceServers, terminals.followTerminalDevice]) spy.mockReset();
  terminals.followTerminalDevice.mockImplementation(() =>
    handedOver.push(contextFor(terminals.deviceId)?.peerLink?.term || null),
  );
  api.fetchIceServers.mockResolvedValue([{ urls: ["stun:stun.test"] }]);
  account.pinnedKey.mockReset();
  account.pinnedKey.mockResolvedValue("pk");
  greetings.greet.mockReset();
  greetings.greet.mockResolvedValue(true);
  linkOpensWith(fakeLink());
});

describe("the connect sequence", () => {
  it("mints over the rendezvous, rides the channels, and closes the relay", async () => {
    const link = fakeLink();
    linkOpensWith(link);

    const context = await connect("dev-a");

    const socket = sockets().at(-1);
    expect(socket.sent[0]).toEqual({ type: "authenticate", token: "tok-1" });
    expect(socket.sent[1].type).toBe("session_init");
    expect(socket.sent[1].route_to).toBe("device:dev-a");
    // Live over the channel, and the relay let go of the moment it carries.
    expect(context.peerLink).toBe(link);
    expect(context.offline).toBe(false);
    expect(socket.readyState).toBe(3);
    // The terminal half is the manager's to take: it is told the device it
    // follows has a link, and reads the channel off that device's context.
    expect(handedOver.at(-1)).toBe(link.term);
  });

  it("greets over the channel, and puts nothing but rtc.* on the relay", async () => {
    const link = fakeLink();
    linkOpensWith(link);

    const context = await connect("dev-a");
    await context.rpc("board.list", {});

    expect(greetings.greet).toHaveBeenCalled();
    expect(overTheRelay()).toEqual(["rtc.offer"]);
    expect(overTheChannel(link.app)).toEqual(["board.list"]);
  });

  it("blocks the device when the ICE servers cannot be minted, and does not try again", async () => {
    api.fetchIceServers.mockRejectedValue(new Error("could not mint ICE servers"));
    peerLink.open.mockImplementation(async ({ fetchIceServers }) => {
      await fetchIceServers();
      throw new Error("unreachable");
    });

    await connect("dev-a");

    const context = contextFor("dev-a");
    expect(context.blocked).toBe("ice-servers");
    expect(context.offline).toBe(true);
    expect(context.peerLink).toBe(null);
    expect(liveSockets()).toEqual([]); // no socket is left holding a blocked device
    expect(peerLink.open).toHaveBeenCalledTimes(1);

    // And asking for every online device again leaves it alone: rule 3 is a
    // Retry or the poll seeing it come back, never a loop.
    await openDeviceSessions().settled;
    expect(peerLink.open).toHaveBeenCalledTimes(1);
  });

  it("blocks the device with no-webrtc when this browser has no peer connection", async () => {
    delete globalThis.RTCPeerConnection;

    await connect("dev-a");

    expect(contextFor("dev-a").blocked).toBe("no-webrtc");
    expect(peerLink.open).not.toHaveBeenCalled();
    expect(sockets()).toEqual([]); // nothing was even asked for it
  });

  it("blocks the device with timeout when the channels do not open in time", async () => {
    peerLink.open.mockRejectedValue(
      Object.assign(new Error("the peer connection did not open its channels"), { blockedReason: "timeout" }),
    );

    await connect("dev-a");

    expect(contextFor("dev-a").blocked).toBe("timeout");
  });

  it("blocks the device with refused when the bridge will not take the offer", async () => {
    FakeWebSocket.refuses = true;

    await connect("dev-a");

    expect(contextFor("dev-a").blocked).toBe("refused");
    expect(liveSockets()).toEqual([]);
  });

  it("blocks the device with unreached when the rendezvous cannot find its bridge", async () => {
    FakeWebSocket.relayDrops = true;

    await connect("dev-a");

    expect(contextFor("dev-a").blocked).toBe("unreached");
    expect(peerLink.open).not.toHaveBeenCalled();
  });

  it("stops for good on a machine whose key is not the one this account pinned", async () => {
    account.pinnedKey.mockResolvedValue(null);

    await connect("dev-a");

    expect(contextFor("dev-a").blocked).toBe("refused");
    expect(securityStopText()).toMatch(/no pinned transport key/);
    const dialled = sockets().length;

    // Not by the poll, and not by a reader pressing Retry: the same pinned key
    // would be offered to the same impostor.
    await openDeviceSessions().settled;
    await connectDevice("dev-a").catch(() => {});
    expect(sockets()).toHaveLength(dialled);
  });
});

describe("a connection that goes after it was live", () => {
  it("blocks the device with lost when a channel closes, and hands both streams back", async () => {
    const link = fakeLink();
    linkOpensWith(link);
    await connect("dev-a");
    handedOver.length = 0;

    link.app.drop();
    await settle();

    const context = contextFor("dev-a");
    expect(context.blocked).toBe("lost");
    expect(context.peerLink).toBe(null);
    expect(handedOver.at(-1)).toBe(null); // the shells have no wire to type down
    expect(link.close).toHaveBeenCalledTimes(1);
  });

  it("blocks it the same way when the terminal half is the one that goes", async () => {
    const link = fakeLink();
    linkOpensWith(link);
    await connect("dev-a");

    link.term.drop();
    await settle();

    expect(contextFor("dev-a").blocked).toBe("lost");
    expect(link.close).toHaveBeenCalledTimes(1);
  });

  it("reopens the rendezvous for an ICE restart, and closes it again once it carries", async () => {
    let askForRestart = null;
    let sayItCarries = null;
    const link = fakeLink();
    peerLink.open.mockImplementation(async ({ fetchIceServers, signal, onConnected, onFailed }) => {
      await fetchIceServers();
      await signal("rtc.offer", { sdp: "v=0" });
      await onConnected();
      askForRestart = onFailed;
      sayItCarries = onConnected;
      return link;
    });
    await connect("dev-a");
    expect(sockets()).toHaveLength(1);

    // The connection failed: the caller reopens the relay and re-attaches this
    // session's signaling before the restart offer is made.
    await askForRestart();
    await settle();

    const reopened = sockets().at(-1);
    expect(sockets()).toHaveLength(2);
    expect(reopened.readyState).toBe(1);
    // The same session id: a carrier re-attach, not a second session.
    expect(reopened.sent[1].session_id).toBe(sockets()[0].sent[1].session_id);

    await sayItCarries();
    expect(reopened.readyState).toBe(3);
  });

  it("keeps an overlapping terminal mint open when a restart finishes", async () => {
    let askForRestart = null;
    let sayItCarries = null;
    const link = fakeLink();
    peerLink.open.mockImplementation(async ({ fetchIceServers, signal, onConnected, onFailed }) => {
      await fetchIceServers();
      await signal("rtc.offer", { sdp: "v=0" });
      askForRestart = onFailed;
      sayItCarries = onConnected;
      return link;
    });
    await connect("dev-a");

    const terminalSession = await terminals.mint("dev-a");
    await askForRestart();
    const sharedSocket = sockets().at(-1);
    await sayItCarries();

    expect(sharedSocket.readyState).toBe(1);
    terminalSession.release();
    expect(sharedSocket.readyState).toBe(3);
  });

  it("keeps the initial lease through a restart that finishes during the greeting", async () => {
    let finishGreeting;
    let askForRestart = null;
    let sayItCarries = null;
    greetings.greet.mockImplementationOnce(
      () => new Promise((resolve) => { finishGreeting = resolve; }),
    );
    peerLink.open.mockImplementationOnce(async ({ signal, onConnected, onFailed }) => {
      await signal("rtc.offer", { sdp: "v=0" });
      askForRestart = onFailed;
      sayItCarries = onConnected;
      return fakeLink();
    });
    App.devices = [{ id: "dev-a", name: "Laptop", status: "online" }];

    const connecting = connectDevice("dev-a");
    await settle();
    await askForRestart();
    const reopened = sockets().at(-1);
    await sayItCarries();

    expect(reopened.readyState).toBe(1);
    finishGreeting(true);
    await connecting;
    expect(reopened.readyState).toBe(3);
  });

  it("ignores restart callbacks captured before force close and a new retry", async () => {
    let staleRestart = null;
    const firstLink = fakeLink();
    peerLink.open.mockImplementationOnce(async ({ signal, onFailed }) => {
      await signal("rtc.offer", { sdp: "v=0" });
      staleRestart = onFailed;
      return firstLink;
    });
    await connect("dev-a");

    goOffline("dev-a");
    const replacement = fakeLink();
    linkOpensWith(replacement);
    await connectDevice("dev-a");
    const opened = sockets().length;

    await expect(staleRestart()).rejects.toThrow(/stale rendezvous restart/);
    expect(sockets()).toHaveLength(opened);
    expect(contextFor("dev-a").peerLink).toBe(replacement);
  });
});

describe("the terminals' session", () => {
  it("is minted on that device's own rendezvous, which closes again after", async () => {
    await connect("dev-a");
    const before = sockets().length;

    const minted = await terminals.mint("dev-a");
    await settle();

    expect(minted.deviceId).toBe("dev-a");
    expect(sockets()).toHaveLength(before + 1); // reopened for the mint…
    expect(sockets().at(-1).readyState).toBe(1); // held until the channel acknowledges it
    minted.release();
    expect(sockets().at(-1).readyState).toBe(3); // …then closed by its lease owner
    // A second session on the same rendezvous, with an id of its own.
    const [appInit] = sockets()[0].sent.filter((message) => message.type === "session_init");
    const [termInit] = sockets().at(-1).sent.filter((message) => message.type === "session_init");
    expect(termInit.session_id).not.toBe(appInit.session_id);
    expect(termInit.route_to).toBe(appInit.route_to); // and to the same machine
    expect(liveSockets()).toEqual([]); // nothing is held open for it (rule 4)
  });
});

describe("one device at a time", () => {
  it("hands the terminal channel over only from the link of the device they follow", async () => {
    const links = { "dev-a": fakeLink(), "dev-b": fakeLink() };
    linkOpensWith((deviceId) => links[deviceId]);

    await connect("dev-a", "dev-b");

    expect(contextFor("dev-a").peerLink).toBe(links["dev-a"]);
    expect(contextFor("dev-b").peerLink).toBe(links["dev-b"]);
    // The terminal stream is one socket on one machine, so the only channel it
    // is ever handed is the one the device it follows owns. The other device's
    // link is news about nothing it rides.
    expect(handedOver.filter(Boolean)).not.toEqual([]);
    expect([...new Set(handedOver.filter(Boolean))]).toEqual([links["dev-a"].term]);
  });

  it("leaves the other device live when one of them is blocked", async () => {
    linkOpensWith((deviceId) => {
      if (deviceId === "dev-b") throw new Error("nothing opened");
      return fakeLink();
    });

    await connect("dev-a", "dev-b");

    expect(contextFor("dev-a").offline).toBe(false);
    expect(contextFor("dev-b").offline).toBe(true);
    expect(contextFor("dev-b").blocked).toBe("failed");
  });
});

describe("device settings connections", () => {
  it("asks over the session the app already holds, and opens no socket of its own", async () => {
    const link = fakeLink();
    linkOpensWith(link);
    const context = await connect("dev-a");
    const opened = sockets().length;

    const settings = await openDeviceSettingsSession("dev-a");
    await settings.call("settings.get", {});

    expect(settings.deviceId).toBe("dev-a");
    expect(overTheChannel(link.app).at(-1)).toBe("settings.get");
    expect(sockets()).toHaveLength(opened);
    settings.close();
    expect(contextFor("dev-a").session).toBe(context.session);
  });

  it("refuses a machine that cannot answer, in the account's words for it", async () => {
    App.devices = [{ id: "dev-z", name: "Studio", status: "offline" }];

    await expect(openDeviceSettingsSession("dev-z")).rejects.toThrow("Studio isn't connected");
  });

  it("tells the page when the machine it is about stops answering", async () => {
    await connect("dev-a");
    const onLost = vi.fn();
    await openDeviceSettingsSession("dev-a", { onLost });

    goOffline("dev-a");

    expect(onLost).toHaveBeenCalledTimes(1);
  });
});

describe("what the live connection tells the surfaces", () => {
  it("applies the live bridge's operation capability to its chat repository", async () => {
    const context = await connect("dev-a");
    greetings.greet.mockImplementationOnce(async (_call, { onGreeting }) => {
      onGreeting({ thread_post_operations: { version: 1, status_method: "thread.operation" } });
      return true;
    });

    await greetLiveBridge(context);

    expect(context.chatRepository.threadPostOperations()).toEqual({
      version: 1,
      statusMethod: "thread.operation",
    });
  });

  it("wakes the surfaces with what the bridge pushed, and keeps the signaling to itself", async () => {
    const link = fakeLink();
    linkOpensWith(link);
    await connect("dev-a");
    changed.length = 0;

    link.app.deliver({ type: "rtc.ice", candidate: { candidate: "candidate:1 1 udp" } });
    link.app.deliver({ type: "entity.changed", id: "run-7" });
    await settle();

    // Every push carries the device it came from; `rtc.ice` belongs to the
    // upgrade that negotiated it and describes nothing the surfaces show.
    expect(changed).toEqual([[{ type: "entity.changed", id: "run-7" }, "dev-a"]]);
  });
});

describe("retrying a blocked device", () => {
  it("runs the connect sequence again and lands it", async () => {
    api.fetchIceServers.mockRejectedValueOnce(new Error("could not mint ICE servers"));
    peerLink.open.mockImplementationOnce(async ({ fetchIceServers }) => {
      await fetchIceServers();
      throw new Error("unreachable");
    });
    await connect("dev-a");
    expect(contextFor("dev-a").blocked).toBe("ice-servers");

    const link = fakeLink();
    linkOpensWith(link);
    await connectDevice("dev-a");
    await settle();

    const context = contextFor("dev-a");
    expect(context.blocked).toBe(null);
    expect(context.offline).toBe(false);
    expect(context.peerLink).toBe(link);
    expect(liveSockets()).toEqual([]); // and the relay is closed again
  });

  it("joins a reader's second press to the attempt already in flight", async () => {
    let land = null;
    peerLink.open.mockImplementation(
      async ({ onConnected }) =>
        new Promise((resolve) => {
          land = async () => {
            await onConnected();
            resolve(fakeLink());
          };
        }),
    );
    App.devices = [{ id: "dev-a", name: "Laptop", status: "online" }];

    const first = connectDevice("dev-a");
    const second = connectDevice("dev-a");
    expect(second).toBe(first);
    await settle();
    await land();
    await first;

    expect(peerLink.open).toHaveBeenCalledTimes(1);
  });
});
