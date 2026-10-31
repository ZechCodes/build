// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// A device's saved transport is the only input the label reads. The fake keeps
// the two calls the label makes and leaves the rest of the module real.
const saved = vi.hoisted(() => {
  const selected = new Map();
  const listeners = new Set();
  return {
    selected,
    listeners,
    choose(deviceId, transport) {
      if (transport) selected.set(deviceId, transport);
      else selected.delete(deviceId);
      for (const listener of [...listeners]) listener();
    },
  };
});
vi.mock("../src/core/httpConnections.js", async (importOriginal) => ({
  ...(await importOriginal()),
  usesHttp: (deviceId) => saved.selected.get(deviceId) === "http",
  onHttpPreferencesChanged: (listener) => {
    saved.listeners.add(listener);
    return () => saved.listeners.delete(listener);
  },
}));

// The shared terminal socket's status, driven by the tests: the label must not
// follow it.
const socket = vi.hoisted(() => {
  const subscribers = new Set();
  return {
    subscribers,
    report(status) { for (const subscriber of [...subscribers]) subscriber(status); },
  };
});
const fakeManager = vi.hoisted(() => ({
  attachTerminal: async () => ({ live: true, snapshot: "", cursor: 0 }),
  attachAgent: async () => ({ term_id: "agent:w1", live: true, snapshot: "", cursor: 0 }),
  input: async () => {},
  resize: async () => {},
  detach: () => {},
}));
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => fakeManager,
  subscribeTerminalStatus: (fn) => {
    socket.subscribers.add(fn);
    return () => socket.subscribers.delete(fn);
  },
  terminalDeviceId: () => null,
  followTerminalDevice: () => {},
  resetTerminalManager: () => {},
  provideTerminalSessions: () => {},
}));
const panes = vi.hoisted(() => ({ mounted: 0 }));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    panes.mounted += 1;
    host.innerHTML = '<div class="term-screen"></div>';
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: () => {} };
  },
}));

import { mountAgentPane, mountAgentTab } from "../src/core/surfaceTabs.js";
import { mountUserTerminalPane } from "../src/core/console.js";

const LABEL = "Terminal over WebRTC";
const labelIn = (host) => host.querySelector(".term-transport");
const paneHost = () => {
  const host = document.createElement("div");
  host.className = "termpane";
  document.body.appendChild(host);
  return host;
};
const agentPane = (deviceId) => {
  const host = paneHost();
  return mountAgentPane(host, { project_id: "p1" }, { deviceId }).then((pane) => ({ host, pane }));
};
const userPane = (deviceId) => {
  const host = paneHost();
  return mountUserTerminalPane(host, "t1", { transportDeviceId: deviceId }).then((pane) => ({ host, pane }));
};

beforeEach(() => {
  saved.selected.clear();
  saved.listeners.clear();
  socket.subscribers.clear();
  panes.mounted = 0;
  document.body.innerHTML = "";
});

describe("a terminal pane names its transport from the device's saved choice", () => {
  it("labels a user terminal and the agent pane of an HTTPS device", async () => {
    saved.choose("dev-http", "http");

    const user = await userPane("dev-http");
    const agent = await agentPane("dev-http");

    expect(labelIn(user.host)?.textContent).toBe(LABEL);
    expect(labelIn(agent.host)?.textContent).toBe(LABEL);
  });

  it("labels nothing on a WebRTC device or one with no saved choice", async () => {
    saved.choose("dev-rtc", "webrtc");

    const chosen = await userPane("dev-rtc");
    const unsaved = await agentPane("dev-none");

    expect(labelIn(chosen.host)).toBeNull();
    expect(labelIn(unsaved.host)).toBeNull();
  });

  it("follows a change of the saved transport without remounting the pane", async () => {
    const { host } = await userPane("dev-a");
    expect(labelIn(host)).toBeNull();

    saved.choose("dev-a", "http");
    expect(labelIn(host)?.textContent).toBe(LABEL);
    saved.choose("dev-a", "webrtc");
    expect(labelIn(host)).toBeNull();
    saved.choose("dev-b", "http");
    expect(labelIn(host)).toBeNull();
    expect(panes.mounted).toBe(1);
  });

  it("stays put whatever the terminal connection reports", async () => {
    saved.choose("dev-http", "http");
    saved.choose("dev-rtc", "webrtc");
    const labelled = await agentPane("dev-http");
    const plain = await agentPane("dev-rtc");

    for (const status of ["disconnected", "connecting", "connected", "disconnected"]) {
      socket.report(status);
      expect(labelIn(labelled.host)?.textContent).toBe(LABEL);
      expect(labelIn(plain.host)).toBeNull();
    }
  });

  it("goes, with its listener, when the pane is disposed", async () => {
    saved.choose("dev-http", "http");
    const user = await userPane("dev-http");
    const agent = await agentPane("dev-http");

    user.pane.dispose();
    agent.pane.dispose();

    expect(labelIn(user.host)).toBeNull();
    expect(labelIn(agent.host)).toBeNull();
    expect(saved.listeners.size).toBe(0);
  });

  it("reaches the agent tab's pane from the device the tab was mounted for", async () => {
    saved.choose("dev-http", "http");
    const host = paneHost();

    const tab = mountAgentTab(host, { project_id: "p1" }, { deviceId: "dev-http" });
    await vi.waitFor(() => expect(labelIn(host.querySelector("#agentpane"))?.textContent).toBe(LABEL));
    tab.dispose();
  });
});
