// @vitest-environment jsdom
// The upgrade policy, in the order the spec sets it out: connect on the relay,
// upgrade in the background, migrate both streams when the two channels open,
// fall back together when they go, and never retry in a loop. With a session per
// device, each device's link is its own — and only the home device's may carry
// the terminals.

import { describe, it, expect, vi, beforeEach } from "vitest";

const peerLink = vi.hoisted(() => ({
  open: vi.fn(),
  close: vi.fn(),
}));
const api = vi.hoisted(() => ({ fetchIceServers: vi.fn(async () => [{ urls: ["stun:stun.test"] }]) }));
const terminals = vi.hoisted(() => ({ terminalsRideOn: vi.fn(), retargetTerminals: vi.fn() }));
const relay = vi.hoisted(() => ({ openRelaySession: vi.fn() }));
const greetings = vi.hoisted(() => ({ greet: vi.fn(async () => true) }));

vi.mock("../src/core/peerLink.js", () => ({
  openPeerLink: (options) => peerLink.open(options),
}));
vi.mock("../src/api.js", () => ({
  fetchIceServers: (...args) => api.fetchIceServers(...args),
  fetchGatewayToken: async () => "tok",
  fetchDevices: async () => [],
}));
vi.mock("../src/terminal/manager.js", () => ({
  terminalsRideOn: (...args) => terminals.terminalsRideOn(...args),
  retargetTerminals: (...args) => terminals.retargetTerminals(...args),
}));
vi.mock("../src/core/session.js", () => ({
  openRelaySession: (options) => relay.openRelaySession(options),
}));
vi.mock("../src/devices.js", () => ({
  deviceName: () => "Machine",
  markDeviceOnline: () => {},
  markDeviceOffline: () => {},
  paintDevicePicker: () => {},
  pinnedDeviceTransportKey: async () => "pk",
  refreshDevices: async () => [],
}));
vi.mock("../src/core/composeView.js", () => ({ flushCaptures: async () => {} }));
vi.mock("../src/core/changeEvents.js", () => ({
  dispatchChangeEvent: (...args) => changed.push(args),
  disarmChangeEvents: () => {},
  greetBridge: (...args) => greetings.greet(...args),
}));
const changed = [];
const { App, disposeApplicationScope } = await import("../src/app.js");
const { claimHomeContext, goOffline, greetLiveBridge, openDeviceSessions, openDeviceSettingsSession } = await import(
  "../src/connection.js"
);

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const settle = async () => {
  await tick();
  await tick();
};

/** A carrier as the policy sees it: something to hand over, and a way to say it
 *  is gone. */
function fakeCarrier(name) {
  let onClose = () => {};
  return { name, onClose: (fn) => (onClose = fn), drop: () => onClose(), send: () => {}, onEnvelope: () => {} };
}

function fakeSession(deviceId = "dev-a") {
  const listeners = new Set();
  return {
    deviceId,
    // Every answer says which device gave it, so an upgrade can tell the two
    // bridges apart over its own signaling channel.
    call: vi.fn(async () => ({ deviceId })),
    peer: vi.fn(),
    onPush: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    push: (payload) => listeners.forEach((fn) => fn(payload)),
    onCarrier: vi.fn(),
    close: vi.fn(),
  };
}

function fakeLink() {
  const link = { app: fakeCarrier("app"), term: fakeCarrier("term"), close: vi.fn() };
  return link;
}

/** What openPeerLink registers: a push handler that takes the candidates and
 *  leaves the rest. The `rtc.ice` shape lives in peerLink, so the policy layer
 *  is exercised through a handler that owns it exactly as the real one does. */
const takeCandidate = (deliver) => (push) => {
  if (push.type === "rtc.ice") deliver(push.candidate);
};

const opened = []; // every relay options bag, in the order the app asked for them

/** Answer each device with the session written for it. */
function relayAnswers(sessions) {
  relay.openRelaySession.mockImplementation(async (options) => {
    opened.push(options);
    const session = sessions.find((one) => one.deviceId === options.preferDeviceId);
    if (!session) throw new Error(`no session for ${options.preferDeviceId}`);
    return session;
  });
}

/** Boot the account: every device online, opened at once, and the first one to
 *  answer is home — what the gate does. */
async function connect(...sessions) {
  App.devices = sessions.map((session) => ({ id: session.deviceId, name: "Machine", status: "online" }));
  relayAnswers(sessions);
  const opening = openDeviceSessions();
  claimHomeContext(await opening.first);
  const contexts = await opening.settled;
  await settle();
  return contexts;
}

/** Lose a live device and let it come back on another session — the one path
 *  that replaces the session a link was opened for. */
async function loseAndReturn(replacement) {
  relayAnswers([replacement]);
  goOffline(replacement.deviceId);
  await settle();
}

beforeEach(() => {
  disposeApplicationScope();
  document.body.innerHTML = '<div id="offbar" hidden><span id="offbar-text"></span></div><div id="conn"></div>';
  document.body.className = "";
  changed.length = 0;
  opened.length = 0;
  App.offline = false;
  App.session = null;
  App.devices = [];
  App.selectedDeviceId = null;
  globalThis.RTCPeerConnection = class {};
  for (const spy of [peerLink.open, api.fetchIceServers, terminals.terminalsRideOn, relay.openRelaySession]) {
    spy.mockReset();
  }
  api.fetchIceServers.mockResolvedValue([{ urls: ["stun:stun.test"] }]);
  greetings.greet.mockReset();
  greetings.greet.mockResolvedValue(true);
});

describe("the upgrade policy", () => {
  it("applies the live bridge's operation capability to its chat repository", async () => {
    delete globalThis.RTCPeerConnection;
    const [context] = await connect(fakeSession());
    greetings.greet.mockImplementationOnce(async (_call, { onGreeting }) => {
      onGreeting({ thread_post_operations: { version: 1, status_method: "thread.operation" } });
      return true;
    });

    await greetLiveBridge(context);

    expect(App.chatRepository.threadPostOperations()).toEqual({
      version: 1,
      statusMethod: "thread.operation",
    });
  });

  it("migrates both streams once the channels are open, and keeps the relay for signaling", async () => {
    const link = fakeLink();
    peerLink.open.mockImplementation(async ({ fetchIceServers, signal }) => {
      await fetchIceServers();
      await signal("rtc.offer", { sdp: "v=0" });
      return link;
    });
    const session = fakeSession();

    await connect(session);

    expect(api.fetchIceServers).toHaveBeenCalledTimes(1);
    // The session routes `rtc.*` to the relay itself: this layer just calls it.
    expect(session.call).toHaveBeenCalledWith("rtc.offer", { sdp: "v=0" });
    expect(session.peer).toHaveBeenCalledWith(link.app);
    expect(terminals.terminalsRideOn).toHaveBeenCalledWith(link.term);
  });

  it("stays on the relay when the ICE servers cannot be minted, and does not try again", async () => {
    api.fetchIceServers.mockRejectedValue(new Error("could not mint ICE servers"));
    peerLink.open.mockImplementation(async ({ fetchIceServers }) => {
      await fetchIceServers();
      throw new Error("unreachable");
    });
    const session = fakeSession();

    await connect(session);

    expect(session.peer).not.toHaveBeenCalled();
    // Nothing was handed a channel to ride — only the release of whatever the
    // session before this one was riding.
    expect(terminals.terminalsRideOn.mock.calls.flat().filter(Boolean)).toEqual([]);
    expect(peerLink.open).toHaveBeenCalledTimes(1); // one attempt per relay session
  });

  it("migrates back to the relay when a channel is lost, taking the other with it", async () => {
    const link = fakeLink();
    peerLink.open.mockResolvedValue(link);
    const session = fakeSession();
    await connect(session);
    session.peer.mockClear();
    terminals.terminalsRideOn.mockClear();

    link.app.drop();

    expect(session.peer).toHaveBeenCalledWith(null);
    expect(terminals.terminalsRideOn).toHaveBeenCalledWith(null);
    expect(link.close).toHaveBeenCalledTimes(1);
  });

  it("gives the peer path up when the session it upgraded is replaced", async () => {
    const link = fakeLink();
    peerLink.open.mockResolvedValue(link);
    await connect(fakeSession());

    peerLink.open.mockResolvedValue(fakeLink());
    await loseAndReturn(fakeSession());

    expect(link.close).toHaveBeenCalledTimes(1);
    expect(terminals.terminalsRideOn).toHaveBeenCalledWith(null);
  });

  it("routes the bridge's trickled candidates to the upgrade, not to the surfaces", async () => {
    const link = fakeLink();
    let deliverCandidate;
    peerLink.open.mockImplementation(async ({ onPush }) => {
      link.close.mockImplementation(onPush(takeCandidate((c) => (deliverCandidate = c))));
      return link;
    });
    const session = fakeSession();
    await connect(session);

    // The session hands every push to both: the link takes the candidates, and
    // the surfaces are told about everything that is not signaling.
    session.push({ type: "rtc.ice", candidate: { candidate: "candidate:1 1 udp" } });
    opened[0].onPush({ type: "rtc.ice", candidate: { candidate: "candidate:1 1 udp" } });
    opened[0].onPush({ type: "entity.changed", id: "run-7" });

    expect(deliverCandidate).toEqual({ candidate: "candidate:1 1 udp" });
    // Every push carries the device it came from: the session was opened for
    // one device, and the surfaces it wakes are that device's.
    expect(changed).toEqual([[{ type: "entity.changed", id: "run-7" }, "dev-a"]]);
  });

  it("does not reach for a peer connection a browser does not have", async () => {
    delete globalThis.RTCPeerConnection;
    await connect(fakeSession());
    expect(peerLink.open).not.toHaveBeenCalled();
  });

  it("migrates back when the terminal's half of the connection is the one that goes", async () => {
    const link = fakeLink();
    peerLink.open.mockResolvedValue(link);
    const session = fakeSession();
    await connect(session);
    session.peer.mockClear();
    terminals.terminalsRideOn.mockClear();

    link.term.drop();

    expect(session.peer).toHaveBeenCalledWith(null);
    expect(terminals.terminalsRideOn).toHaveBeenCalledWith(null);
    expect(link.close).toHaveBeenCalledTimes(1);
  });

  it("keeps each upgrade's candidates its own when one overtakes another", async () => {
    const links = [fakeLink(), fakeLink()];
    const delivered = [];
    let opening = 0;
    const sessions = [fakeSession(), fakeSession()];
    peerLink.open.mockImplementation(async ({ onPush }) => {
      const mine = opening++;
      // A real link gives its subscription back when it is torn down, and takes
      // back its own and nobody else's.
      links[mine].close.mockImplementation(onPush(takeCandidate((c) => delivered.push([mine, c]))));
      return links[mine];
    });

    await connect(sessions[0]);
    await loseAndReturn(sessions[1]); // the first link is torn down as this one is adopted

    sessions[0].push({ type: "rtc.ice", candidate: { candidate: "candidate:9 1 udp" } });
    sessions[1].push({ type: "rtc.ice", candidate: { candidate: "candidate:9 1 udp" } });
    expect(delivered).toEqual([[1, { candidate: "candidate:9 1 udp" }]]);
  });

  it("hands the terminal channel over only from the home device's link", async () => {
    const links = { "dev-a": fakeLink(), "dev-b": fakeLink() };
    peerLink.open.mockImplementation(async ({ signal }) => links[(await signal("rtc.hello", {})).deviceId]);
    const home = fakeSession("dev-a");
    const other = fakeSession("dev-b");

    await connect(home, other);

    // Both devices ride their own app channel; the terminal stream is one
    // socket on one machine, so only home's term channel is handed over.
    expect(home.peer).toHaveBeenCalledWith(links["dev-a"].app);
    expect(other.peer).toHaveBeenCalledWith(links["dev-b"].app);
    const handedOver = terminals.terminalsRideOn.mock.calls.flat().filter(Boolean);
    expect(handedOver).toHaveLength(1);
    expect(handedOver[0]).toBe(links["dev-a"].term);
  });
});

describe("device settings connections", () => {
  it("opens the named device without adopting it or inheriting the active device's offline state", async () => {
    const current = fakeSession();
    const settings = { ...fakeSession(), deviceId: "dev-b" };
    App.session = current;
    App.offline = true;
    relay.openRelaySession.mockResolvedValueOnce(settings);
    const onLost = vi.fn();
    await expect(openDeviceSettingsSession("dev-b", { onLost })).resolves.toBe(settings);
    const options = relay.openRelaySession.mock.calls[0][0];
    expect(options.preferDeviceId).toBe("dev-b");
    expect(options.waitForDevice).toBe(false);
    expect(options.getPinnedDeviceKey).toBeTypeOf("function");
    expect(options.isPaused).toBeUndefined();
    expect(options.onLost).toBe(onLost);
    expect(App.session).toBe(current);
    expect(App.offline).toBe(true);
  });
  it("closes a mismatched session before exposing any RPC", async () => {
    const wrong = fakeSession();
    relay.openRelaySession.mockResolvedValueOnce(wrong);
    await expect(openDeviceSettingsSession("dev-b")).rejects.toThrow("requested device");
    expect(wrong.close).toHaveBeenCalledOnce();
    expect(wrong.call).not.toHaveBeenCalled();
  });
});
