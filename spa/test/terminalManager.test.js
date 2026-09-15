// Who hears a channel close, and which machine the one terminal socket is on.
//
// "The two channels are one connection and fall back together" is written in
// connection.js, which registers on both halves and hands both streams back at
// once. The manager is the setter it hands the terminal's half to — a second
// listener here would run the same fallback twice, through two owners of one
// fact.
//
// Which machine the shells type at is this file's own question, asked once by
// terminalDeviceId(): the route's device while a link names one, else the home
// device. Nobody else compares device ids.

import { beforeEach, describe, it, expect, vi } from "vitest";

const sockets = vi.hoisted(() => []);
const contexts = vi.hoisted(() => new Map());

vi.mock("@build/secure-transport", () => ({ ready: async () => {} }));
vi.mock("../src/config.js", () => ({ RELAY_URL: "wss://relay.test" }));
vi.mock("../src/app.js", () => ({ App: { route: { name: "inbox" }, devices: [], selectedDeviceId: null } }));
vi.mock("../src/api.js", () => ({ fetchGatewayToken: async () => "tok" }));
vi.mock("../src/devices.js", () => ({ pinnedDeviceTransportKey: async () => "pk" }));
// The registry is stood in for; what counts as a machine that can answer is
// not — that question has one answer, and this file reads the real one.
vi.mock("../src/core/deviceContexts.js", async () => ({
  ...(await vi.importActual("../src/core/deviceContexts.js")),
  contextFor: (deviceId) => contexts.get(deviceId) || null,
}));
// The socket itself is another file's subject: what matters here is which
// device it was told to want, what it was handed to ride, and whether it was
// dropped so it can re-read the first of those.
vi.mock("../src/terminal/session.js", () => ({
  TerminalSocket: class {
    constructor(options) {
      this.options = options;
      this.deviceId = options.preferDeviceId();
      this.drops = 0;
      this.carriers = [];
      sockets.push(this);
    }
    onStatus() {}
    async start() {}
    peer(carrier) {
      this.carriers.push(carrier);
    }
    simulateDrop() {
      this.drops += 1;
    }
  },
}));

const { App } = await import("../src/app.js");
const { followTerminalDevice, terminalManager } = await import("../src/terminal/manager.js");

/** The one socket the manager owns, as if it had connected to `deviceId`, with
 *  its counters cleared. */
function socketOn(deviceId) {
  const socket = terminalManager();
  socket.deviceId = deviceId;
  socket.drops = 0;
  socket.carriers.length = 0;
  return socket;
}

const online = (id) => ({ id, status: "online" });

/** A machine with an open session, the way the connection registers one. */
const live = (deviceId, extra = {}) => contexts.set(deviceId, { deviceId, call: async () => ({}), ...extra });

beforeEach(() => {
  App.devices = [online("dev-a"), online("dev-b")];
  App.selectedDeviceId = "dev-a";
  App.route = { name: "inbox" };
  contexts.clear();
});

describe("the carrier the terminals are given", () => {
  it("is watched by nobody here", () => {
    const term = { onClose: vi.fn(), onEnvelope: vi.fn(), send: vi.fn(), close: vi.fn() };
    live("dev-a", { peerLink: { term } });
    socketOn("dev-a");

    followTerminalDevice();

    expect(term.onClose).not.toHaveBeenCalled();
  });
});

describe("the device the terminals follow", () => {
  it("with no route device the terminals follow the home device", () => {
    live("dev-a");
    const socket = socketOn("dev-a");

    expect(socket.options.preferDeviceId()).toBe("dev-a");

    App.selectedDeviceId = "dev-b";

    expect(socket.options.preferDeviceId()).toBe("dev-b");

    App.route = { name: "branch", deviceId: "dev-a", projectId: "p1" };

    expect(socket.options.preferDeviceId()).toBe("dev-a"); // the link wins
  });

  it("a route change to another device drops the socket once", () => {
    live("dev-b");
    const socket = socketOn("dev-a");
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };

    followTerminalDevice();

    expect(socket.drops).toBe(1);
    expect(socket.options.preferDeviceId()).toBe("dev-b"); // what it re-reads as it comes back
  });

  it("the same device does not drop it", () => {
    live("dev-b");
    const socket = socketOn("dev-b");
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };

    followTerminalDevice();

    expect(socket.drops).toBe(0);
  });

  // A link to a machine this client has never opened mounts a notice, not a
  // surface, and no shell types at a machine that is not there: the terminals
  // stay on the home device rather than being dropped onto an empty one.
  it("leaves them on the home device when the route's machine was never opened here", () => {
    live("dev-a");
    const socket = socketOn("dev-a");
    App.route = { name: "branch", deviceId: "dev-c", projectId: "p1" }; // never opened here

    followTerminalDevice();

    expect(socket.drops).toBe(0);
    expect(socket.options.preferDeviceId()).toBe("dev-a");
  });

  // A machine that has answered before is still the machine the work is on: an
  // outage must not move its shells to another machine under a surface about
  // this one. They stay put, and the socket's own reconnect brings them back
  // here when the machine does.
  it("keeps them on a machine that has gone offline since", () => {
    live("dev-a");
    live("dev-b", { offline: true });
    const socket = socketOn("dev-b");
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };

    followTerminalDevice();

    expect(socket.drops).toBe(0);
    expect(socket.options.preferDeviceId()).toBe("dev-b");
  });

  // A machine this client HAS opened and cannot reach right now is the machine
  // the link is about, so the shells go there — but not yet. Dropping the socket
  // onto it re-attaches every open tab against a machine that cannot answer, and
  // the tabs come back empty; they stay where they are until it can.
  it("does not move the socket onto a known device that is offline when the link arrives", () => {
    live("dev-a");
    live("dev-b", { offline: true });
    const socket = socketOn("dev-a");
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };

    expect(followTerminalDevice()).toBe(false);

    expect(socket.drops).toBe(0);
    expect(socket.carriers).toEqual([]); // nor is dev-a taken off its own carrier
  });

  it("followTerminalDevice hands over the route device's peer term channel", () => {
    const term = { id: "term-b" };
    live("dev-a", { peerLink: { term: { id: "term-a" } } });
    live("dev-b", { peerLink: { term } });
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };
    const socket = socketOn("dev-b");

    followTerminalDevice();

    expect(socket.carriers.at(-1)).toBe(term);
  });

  it("takes the terminals off a peer channel the device they follow does not own", () => {
    live("dev-a", { peerLink: { term: { id: "term-a" } } });
    live("dev-b");
    App.route = { name: "branch", deviceId: "dev-b", projectId: "p1" };
    const socket = socketOn("dev-b");

    followTerminalDevice();

    expect(socket.carriers.at(-1)).toBe(null);
  });
});
