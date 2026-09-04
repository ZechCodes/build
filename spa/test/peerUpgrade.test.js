// @vitest-environment jsdom
// The upgrade policy, in the order the spec sets it out: connect on the relay,
// upgrade in the background, migrate both streams when the two channels open,
// fall back together when they go, and never retry in a loop.

import { describe, it, expect, vi, beforeEach } from "vitest";

const peerLink = vi.hoisted(() => ({
  open: vi.fn(),
  close: vi.fn(),
}));
const api = vi.hoisted(() => ({ fetchIceServers: vi.fn(async () => [{ urls: ["stun:stun.test"] }]) }));
const terminals = vi.hoisted(() => ({ terminalsRideOn: vi.fn(), retargetTerminals: vi.fn() }));
const relay = vi.hoisted(() => ({ openRelaySession: vi.fn() }));

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
  greetBridge: async () => ({}),
}));
vi.mock("../src/core/cacheScope.js", () => ({ setCacheDevice: () => {} }));

const changed = [];
const { App } = await import("../src/app.js");
const { adoptSession, openAppSession } = await import("../src/connection.js");

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A carrier as the policy sees it: something to hand over, and a way to say it
 *  is gone. */
function fakeCarrier(name) {
  let onClose = () => {};
  return { name, onClose: (fn) => (onClose = fn), drop: () => onClose(), send: () => {}, onEnvelope: () => {} };
}

function fakeSession() {
  return {
    deviceId: "dev-a",
    call: vi.fn(),
    signal: vi.fn(async () => ({})),
    peer: vi.fn(),
    onCarrier: vi.fn(),
    close: vi.fn(),
  };
}

function fakeLink() {
  const link = { app: fakeCarrier("app"), term: fakeCarrier("term"), close: vi.fn() };
  return link;
}

beforeEach(() => {
  document.body.innerHTML = '<div id="offbar"><span id="offbar-text"></span></div><div id="conn"></div>';
  changed.length = 0;
  App.offline = false;
  App.session = null;
  globalThis.RTCPeerConnection = class {};
  for (const spy of [peerLink.open, api.fetchIceServers, terminals.terminalsRideOn, relay.openRelaySession]) {
    spy.mockReset();
  }
  api.fetchIceServers.mockResolvedValue([{ urls: ["stun:stun.test"] }]);
});

describe("the upgrade policy", () => {
  it("migrates both streams once the channels are open, and keeps the relay for signaling", async () => {
    const link = fakeLink();
    peerLink.open.mockImplementation(async ({ fetchIceServers, signal }) => {
      await fetchIceServers();
      await signal("rtc.offer", { sdp: "v=0" });
      return link;
    });
    const session = fakeSession();

    adoptSession(session);
    await tick();

    expect(api.fetchIceServers).toHaveBeenCalledTimes(1);
    expect(session.signal).toHaveBeenCalledWith("rtc.offer", { sdp: "v=0" });
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

    adoptSession(session);
    await tick();
    await tick();

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
    adoptSession(session);
    await tick();
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
    adoptSession(fakeSession());
    await tick();

    peerLink.open.mockResolvedValue(fakeLink());
    adoptSession(fakeSession());

    expect(link.close).toHaveBeenCalledTimes(1);
    expect(terminals.terminalsRideOn).toHaveBeenCalledWith(null);
  });

  it("routes the bridge's trickled candidates to the upgrade, not to the surfaces", async () => {
    const link = fakeLink();
    let deliverCandidate;
    peerLink.open.mockImplementation(async ({ remoteCandidates }) => {
      remoteCandidates((candidate) => (deliverCandidate = candidate));
      return link;
    });
    const session = fakeSession();
    relay.openRelaySession.mockImplementation(async () => session);
    const options = [];
    relay.openRelaySession.mockImplementation(async (o) => {
      options.push(o);
      return session;
    });
    await openAppSession();
    adoptSession(session);
    await tick();

    options[0].onPush({ type: "rtc.ice", candidate: { candidate: "candidate:1 1 udp" } });
    options[0].onPush({ type: "entity.changed", id: "run-7" });

    expect(deliverCandidate).toEqual({ candidate: "candidate:1 1 udp" });
    expect(changed).toEqual([[{ type: "entity.changed", id: "run-7" }]]);
  });

  it("does not reach for a peer connection a browser does not have", async () => {
    delete globalThis.RTCPeerConnection;
    adoptSession(fakeSession());
    await tick();
    expect(peerLink.open).not.toHaveBeenCalled();
  });
});
