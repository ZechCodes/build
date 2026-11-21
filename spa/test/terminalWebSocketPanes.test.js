// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/api/http/terminal-websocket-v1.json";
import { createBrowserHttpEndpoint } from "../src/core/httpBrowserEndpoint.js";
import { clearTerminalPeerLinks, configureTerminalPeerLinks } from "../src/core/terminalPeerLinks.js";
import { mountUserTerminalPane } from "../src/core/console.js";
import { mountAgentPane } from "../src/core/surfaceTabs.js";
import { followTerminalDevice, provideTerminalSessions, resetTerminalManager, terminalManager } from "../src/terminal/manager.js";

const { App, contexts, emulators } = vi.hoisted(() => ({
  App: { route: {}, devices: [], selectedDeviceId: null },
  contexts: new Map(),
  emulators: [],
}));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/deviceContexts.js", async (original) => ({
  ...(await original()), contextFor: (id) => contexts.get(id) || null,
}));
vi.mock("ghostty-web", () => ({
  init: async () => {},
  FitAddon: class { fit() {} },
  Terminal: class {
    cols = 80;
    rows = 24;
    writes = [];
    open() { emulators.push(this); }
    reset() {}
    write(bytes, applied) { this.writes.push({ bytes, applied }); }
    onData(fn) { this.input = fn; }
    onResize(fn) { this.resize = fn; }
    loadAddon() {}
    dispose() {}
  },
}));

const frame = (name) => JSON.parse(fixture.messages[name].utf8);
const epoch = fixture.opening.base.greeting.bridge_epoch;
const json = (value, status = 200) => new Response(JSON.stringify(value), {
  status, headers: fixture.ticket.response.headers,
});
const encode = new TextEncoder();
const decode = new TextDecoder();
const opens = [];
const sockets = [];
const rtcParent = vi.fn();
const rtcMint = vi.fn();

class Peer extends EventTarget {
  readyState = 0;
  bufferedAmount = 0;
  extensions = "";
  protocol = fixture.handshake.selected;
  frames = [];
  close = vi.fn(() => { this.readyState = 3; });
  connect() { this.readyState = 1; this.dispatchEvent(new Event("open")); }
  drop() { this.readyState = 3; this.dispatchEvent(Object.assign(new Event("close"), { code: 1006 })); }
  send(bytes) { this.frames.push(JSON.parse(decode.decode(bytes))); }
  take(method) { return this.frames.filter((value) => value.method === method); }
  receive(value) {
    this.dispatchEvent(new MessageEvent("message", { data: encode.encode(JSON.stringify(value)).buffer }));
  }
  reply(call, message = { bridge_epoch: epoch, ok: true, result: {} }) {
    this.receive({ ...message, id: call.id });
  }
}

async function urlDevice(id, { refusal, mode = "wss" } = {}) {
  const { endpoint: policy, opening, ticket } = fixture;
  const now = Date.parse(ticket.lifetime_cases[0].issued_at);
  const fetchImpl = vi.fn(async (url) => {
    if (url.endsWith(ticket.path)) return refusal ? json(refusal.body, refusal.status) : json(ticket.response.body, 201);
    return json({ ...opening.base, grant_expires_at: new Date(now + 60000).toISOString() });
  });
  const endpoint = createBrowserHttpEndpoint({
    grant: {
      identity: { origin: policy.bridge_https_origin }, token: () => "fixture-bearer",
      expiresAt: () => now + 30000, renew: async () => {}, close() {},
    },
    fetchImpl, now: () => now, callerOrigin: policy.caller_origin,
    createWebSocket: () => {
      const socket = new Peer();
      sockets.push(socket);
      return socket;
    },
  });
  opens.push(endpoint);
  const held = await endpoint.open("range");
  const rest = vi.fn(async () => ({ terminals: [{ term_id: "term-1" }] }));
  const session = {
    sessionId: held.session_id, transport: "http", terminalTransport: mode,
    openTerminal: vi.fn((options) => endpoint.openTerminal(held, options)),
  };
  const context = { deviceId: id, session, rpc: rest, call: rest };
  contexts.set(id, context);
  return { session, fetchImpl, rest };
}

function select(id) {
  App.route = { deviceId: id };
  followTerminalDevice();
}

async function openPeer(index = 0) {
  await vi.waitFor(() => expect(sockets[index]).toBeDefined());
  sockets[index].connect();
  return sockets[index];
}

async function answer(peer, method, name) {
  await vi.waitFor(() => expect(peer.take(method).length).toBeGreaterThan(0));
  const call = peer.take(method).at(-1);
  peer.reply(call, frame(name));
  return call;
}

function host() {
  const node = document.createElement("div");
  document.body.append(node);
  return node;
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  configureTerminalPeerLinks({ open: rtcParent, isCurrent: (context) => contexts.get(context.deviceId) === context });
  provideTerminalSessions(rtcMint);
});

afterEach(() => {
  resetTerminalManager();
  clearTerminalPeerLinks();
  opens.splice(0).forEach((endpoint) => endpoint.close());
  contexts.clear();
  sockets.length = 0;
  emulators.length = 0;
  document.body.replaceChildren();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("URL terminal panes", () => {
  it("uses one WSS socket for shell and agent panes, with zero relay or RTC attempts and sibling input during a delayed render ACK", async () => {
    const { session, rest } = await urlDevice("A");
    select("A");
    const manager = terminalManager();
    expect(sockets).toHaveLength(0);
    const roster = await manager.listTerminals({});
    expect(roster).toEqual([{ term_id: "term-1" }]);
    expect(rest).toHaveBeenCalledWith("term.list", {});
    expect(sockets).toHaveLength(0);

    const shell = mountUserTerminalPane(host(), roster[0].term_id, { deviceId: "A" });
    const agent = mountAgentPane(host(), frame("agent_attach").params, { deviceId: "A" });
    const peer = await openPeer();
    const shellAttach = await answer(peer, "term.attach", "attach_ok");
    await answer(peer, "agent.attach", "agent_attach_ok");
    const [shellPane, agentPane] = await Promise.all([shell, agent]);

    expect(session.openTerminal).toHaveBeenCalledOnce();
    expect(sockets).toHaveLength(1);
    expect(rtcParent).not.toHaveBeenCalled();
    expect(rtcMint).not.toHaveBeenCalled();
    expect(peer.take("term.ack")).toEqual([]);
    agentPane.terminal.writes[0].applied();
    expect(peer.take("term.ack").map((call) => call.params.term_id)).toEqual(["agent:agent-1"]);
    peer.reply(peer.take("term.ack")[0]);

    agentPane.terminal.input("x");
    agentPane.terminal.resize({ cols: 100, rows: 30 });
    expect(peer.frames.slice(-2).map((call) => call.method)).toEqual(["term.input", "term.resize"]);
    expect(peer.take("term.input")[0].params).toEqual({ term_id: "agent:agent-1", data: "eA==" });
    peer.reply(peer.take("term.input")[0]);
    peer.reply(peer.take("term.resize")[0]);
    shellPane.terminal.writes[0].applied();
    expect(peer.take("term.ack").at(-1).params).toEqual({ cursor: 9, term_id: "term-1" });

    shellPane.dispose();
    expect(peer.frames.at(-1)).toEqual({ type: "detach", attach_id: shellAttach.id });
    expect(peer.close).not.toHaveBeenCalled();
    agentPane.dispose();
    expect(peer.close).toHaveBeenCalledOnce();
  });

  it("shows the ticket POST 429 in the pane and keeps the roster on REST", async () => {
    const refusal = Object.values(fixture.ticket.refusals).find(({ status }) => status === 429);
    const { rest } = await urlDevice("A", { refusal });
    select("A");
    const paneHost = host();
    const pane = await mountUserTerminalPane(paneHost, "term-1", { deviceId: "A" });
    expect(paneHost.textContent).toContain("Terminal connection limit reached");
    await expect(terminalManager().listTerminals({})).resolves.toHaveLength(1);
    expect(rest).toHaveBeenCalledOnce();
    expect(sockets).toHaveLength(0);
    expect(rtcParent).not.toHaveBeenCalled();
    pane.dispose();
  });

  it("restores a shell from a fresh ticket, accepts equal-cursor resets and never replays input", async () => {
    const { session } = await urlDevice("A");
    select("A");
    const output = vi.fn();
    const snapshot = vi.fn();
    const manager = terminalManager();
    const attached = manager.attachTerminal("term-1", {}, { onSnapshot: snapshot, onOutput: output });
    const first = await openPeer();
    await answer(first, "term.attach", "attach_ok");
    await attached;
    const input = manager.input("term-1", "x");
    const lost = expect(input).rejects.toMatchObject({ uncertain: true });
    first.drop();
    await lost;
    const second = await openPeer(1);
    await answer(second, "term.attach", "attach_ok");
    const initial = frame("attach_ok").result;
    second.receive({ ...frame("reset"), cursor: initial.cursor, data: initial.snapshot });
    second.receive(frame("output"));
    await vi.waitFor(() => expect(output).toHaveBeenCalledOnce());
    expect(snapshot).toHaveBeenCalledTimes(3);
    expect(second.take("term.input")).toEqual([]);
    expect(session.openTerminal).toHaveBeenCalledTimes(2);
  });

  it("keeps an oversized screen pane-local until the user requests a fresh attach", async () => {
    await urlDevice("A");
    select("A");
    const paneHost = host();
    const mounting = mountUserTerminalPane(paneHost, "term-1", { deviceId: "A" });
    const sibling = terminalManager().attachAgent(frame("agent_attach").params);
    const first = await openPeer();
    await answer(first, "term.attach", "oversized_attach");
    await answer(first, "agent.attach", "agent_attach_ok");
    const pane = await mounting;
    await sibling;
    expect(paneHost.textContent).toContain("This screen is too large to display");
    first.drop();
    const second = await openPeer(1);
    await answer(second, "agent.attach", "agent_attach_ok");
    expect(second.take("term.attach")).toEqual([]);
    paneHost.querySelector("button").click();
    await answer(second, "term.attach", "attach_ok");
    await vi.waitFor(() => expect(paneHost.textContent).not.toContain("This screen is too large to display"));
    pane.dispose();
  });

  it("fences the old device during a pending attach and drops its late output and input", async () => {
    await urlDevice("A");
    await urlDevice("B");
    select("A");
    const manager = terminalManager();
    const snapshot = vi.fn();
    const pending = manager.attachTerminal("term-1", {}, { onSnapshot: snapshot });
    const canceled = expect(pending).rejects.toBeDefined();
    const first = await openPeer();
    await vi.waitFor(() => expect(first.take("term.attach")).toHaveLength(1));
    select("B");
    expect(first.close).toHaveBeenCalledOnce();
    first.reply(first.take("term.attach")[0], frame("attach_ok"));
    await canceled;
    await expect(manager.input("term-1", "old")).rejects.toBeDefined();
    expect(snapshot).not.toHaveBeenCalled();
    expect(sockets).toHaveLength(1);
  });
});
