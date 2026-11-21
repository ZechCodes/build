// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/api/http/terminal-websocket-v1.json";
import manifest from "../../fixtures/api/http/resource-manifest.json";
import { readCached, writeCached } from "../src/core/localCache.js";
import { createHttpRoutes } from "../src/core/httpRoutes.js";
import { openHttpSession } from "../src/core/httpSession.js";
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
const routes = createHttpRoutes(manifest);
const rosterPath = routes.request("term.list").path;
const json = (value, status = 200) => new Response(JSON.stringify(value), {
  status, headers: fixture.ticket.response.headers,
});
const encode = new TextEncoder();
const decode = new TextDecoder();
const opens = [];
const sockets = [];
const rtcParent = vi.fn();
const rtcMint = vi.fn();
const relaySocket = vi.fn();
const rtcPeer = vi.fn();

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
    const bytes = new Uint8Array(encode.encode(JSON.stringify(value)));
    this.dispatchEvent(new MessageEvent("message", { data: bytes.buffer }));
  }
  reply(call, message = { bridge_epoch: epoch, ok: true, result: {} }) {
    this.receive({ ...message, id: call.id });
  }
}

async function urlDevice(id, { refusal, mode = "v1" } = {}) {
  const { endpoint: policy, opening, ticket } = fixture;
  const now = Date.parse(ticket.lifetime_cases[0].issued_at);
  const vector = opening.cases.find(({ name }) => name === mode);
  const advertised = structuredClone(opening.base);
  advertised.transport.operation_ids = routes.operationIds();
  if (vector.descriptor) advertised.transport.terminal_websocket = vector.descriptor;
  if (vector.capability) advertised.greeting.capabilities.push(opening.capability);
  const terminals = [{ term_id: "term-1" }];
  const fetchImpl = vi.fn(async (url) => {
    if (url.endsWith(ticket.path)) return refusal ? json(refusal.body, refusal.status) : json(ticket.response.body, 201);
    if (url.endsWith(rosterPath)) return json({ bridge_epoch: epoch, ok: true, result: { terminals } });
    return json({ ...advertised, grant_expires_at: new Date(now + 60000).toISOString() });
  });
  const endpoint = createBrowserHttpEndpoint({
    grant: {
      identity: { origin: policy.bridge_https_origin, deviceId: id }, token: () => "fixture-bearer",
      expiresAt: () => now + 30000, renew: async () => {}, close() {},
    },
    fetchImpl, now: () => now, callerOrigin: policy.caller_origin,
    createWebSocket: () => {
      const socket = new Peer();
      sockets.push(socket);
      return socket;
    },
  });
  const session = await openHttpSession({
    endpoint: { ...endpoint, async subscribe(held, take) {
      await take({ event: "hello", data: {
        durable: false, recovery: "current_state_refresh", bridge_epoch: epoch, opening_generation: held.opening_generation,
      } });
      return { binding: "fixture-subscription", close() {} };
    } },
    routes,
  });
  opens.push(session);
  session.allowRefresh();
  vi.spyOn(session, "openTerminal");
  const rest = vi.fn((method, params) => session.rawCall(method, params));
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
  await vi.waitFor(() => expect(peer.take(method).length, method).toBeGreaterThan(0));
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
  vi.stubGlobal("WebSocket", relaySocket);
  vi.stubGlobal("RTCPeerConnection", rtcPeer);
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
    expect(session.terminalTransport).toBe("wss");
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
    expect(relaySocket).not.toHaveBeenCalled();
    expect(rtcPeer).not.toHaveBeenCalled();
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
    await vi.waitFor(() => expect(second.take("agent.attach"), JSON.stringify(second.frames)).toHaveLength(1));
    await answer(second, "agent.attach", "agent_attach_ok");
    expect(second.take("term.attach")).toEqual([]);
    paneHost.querySelector(".term-conn button").click();
    await answer(second, "term.attach", "attach_ok");
    await vi.waitFor(() => expect(paneHost.textContent).not.toContain("This screen is too large to display"));
    pane.dispose();
  });

  it("uses RTC for a future-version Opening with a declared legacy fallback", async () => {
    const { session } = await urlDevice("A", { mode: "future_version" });
    select("A");
    terminalManager();
    await vi.waitFor(() => expect(rtcMint).toHaveBeenCalledOnce());
    expect(session.terminalTransport).toBe("webrtc");
    expect(session.openTerminal).not.toHaveBeenCalled();
    expect(sockets).toHaveLength(0);
  });

  it("retires a carrier held open by a bridge that leaves every call slot busy", async () => {
    vi.useFakeTimers();
    await urlDevice("A");
    select("A");
    const manager = terminalManager();
    const attached = manager.attachTerminal("term-1");
    const first = await openPeer();
    await answer(first, "term.attach", "attach_ok");
    await attached;
    first.reply(first.take("term.ack")[0]);
    const effects = Array.from({ length: fixture.limits.outstanding_calls.socket }, (_, i) =>
      manager.resize("term-1", 80 + i, 24).catch((error) => error));
    expect(first.take("term.resize")).toHaveLength(8);

    await vi.advanceTimersByTimeAsync(9000);
    expect(first.close).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1250);
    expect(first.close).toHaveBeenCalledOnce();
    expect(await Promise.all(effects)).toEqual(Array(8).fill(expect.objectContaining({ uncertain: true })));
    const second = await openPeer(1);
    await answer(second, "term.attach", "attach_ok");
    expect(second.take("term.resize")).toEqual([]);
  });

  it("holds a pre-attach large reset separately and ACKs it after the hidden emulator renders", async () => {
    await urlDevice("A");
    select("A");
    const mounted = mountUserTerminalPane(host(), "term-1", { deviceId: "A" });
    const peer = await openPeer();
    await vi.waitFor(() => expect(peer.take("term.attach")).toHaveLength(1));
    const large = frame("reset");
    large.data = btoa("x".repeat(fixture.limits.snapshot_bound.raw_bytes));
    expect(large.data.length).toBeGreaterThan(fixture.limits.message_bytes.ordinary);
    peer.receive(large);
    peer.receive({ ...frame("output"), cursor: large.cursor + 3 });
    await answer(peer, "term.attach", "attach_ok");
    const pane = await mounted;
    expect(pane.terminal.writes).toHaveLength(1);
    pane.terminal.writes[0].applied();
    expect(pane.terminal.writes[1].bytes.length).toBe(fixture.limits.snapshot_bound.raw_bytes);
    await new Promise((resolve) => setTimeout(resolve, 275));
    expect(peer.take("term.ack").map(({ params }) => params.cursor)).toEqual([9]);
    pane.terminal.writes[1].applied();
    expect(pane.terminal.writes[2].bytes).toEqual(new Uint8Array([13, 10, 88]));
    pane.terminal.writes[2].applied();
    await vi.waitFor(() => expect(peer.take("term.ack").at(-1).params.cursor).toBe(15));
    expect(peer.take("term.attach")).toHaveLength(1);
    expect(sockets).toHaveLength(1);
    pane.dispose();
  });

  it("rekeys an agent pane from a pending birth and accepts the next keystroke", async () => {
    await urlDevice("A");
    select("A");
    const manager = terminalManager();
    const onTermId = vi.fn();
    const onLive = vi.fn();
    const attach = manager.attachAgent(frame("agent_attach").params, { onTermId, onLive });
    const peer = await openPeer();
    await vi.waitFor(() => expect(peer.take("agent.attach")).toHaveLength(1));
    const result = { ...frame("agent_attach_ok").result, term_id: "agent:worktree-1", live: false, provider: null };
    peer.reply(peer.take("agent.attach")[0], { ...frame("agent_attach_ok"), result });
    await attach;
    peer.receive({ ...frame("reset"), term_id: "agent:newborn" });
    expect(onTermId.mock.calls.map(([termId]) => termId)).toEqual(["agent:worktree-1", "agent:newborn"]);
    expect(onLive).toHaveBeenLastCalledWith(true);
    const input = manager.input("agent:newborn", "x");
    expect(peer.take("term.input")[0].params.term_id).toBe("agent:newborn");
    peer.reply(peer.take("term.input")[0]);
    await input;
  });

  it("keeps a mounted pane across replacement of its device Opening during attach", async () => {
    await urlDevice("A");
    select("A");
    const snapshot = vi.fn();
    const mounting = terminalManager().attachTerminal("term-1", {}, { onSnapshot: snapshot });
    const first = await openPeer();
    await vi.waitFor(() => expect(first.take("term.attach")).toHaveLength(1));

    await urlDevice("A");
    select("A");
    expect(first.close).toHaveBeenCalledOnce();
    await mounting;
    expect(snapshot).not.toHaveBeenCalled();
    const second = await openPeer(1);
    await answer(second, "term.attach", "attach_ok");
    await vi.waitFor(() => expect(snapshot).toHaveBeenCalledOnce());
  });

  it("resnapshots only the shell with a cursor gap and honors its later attachment refusal", async () => {
    await urlDevice("A");
    select("A");
    const manager = terminalManager();
    const shellFailure = vi.fn();
    const shell = manager.attachTerminal("term-1", {}, { onFailure: shellFailure });
    const agent = manager.attachAgent(frame("agent_attach").params);
    const peer = await openPeer();
    const oldShell = await answer(peer, "term.attach", "attach_ok");
    await answer(peer, "agent.attach", "agent_attach_ok");
    await Promise.all([shell, agent]);

    peer.receive({ ...frame("output"), cursor: 18 });
    await vi.waitFor(() => expect(peer.take("term.attach")).toHaveLength(2));
    expect(peer.frames).toContainEqual({ ...frame("detach"), attach_id: oldShell.id });
    expect(peer.take("agent.attach")).toHaveLength(1);
    const replacement = await answer(peer, "term.attach", "attach_ok");
    peer.receive({ ...frame("attachment_error"), attach_id: replacement.id });
    expect(shellFailure).toHaveBeenLastCalledWith({
      message: "This screen is too large to display", retry: expect.any(Function),
    });
    const input = manager.input("agent:agent-1", "x");
    peer.reply(peer.take("term.input")[0]);
    await input;
    expect(peer.close).not.toHaveBeenCalled();
  });

  it("reconciles uncertain create and close through the cached REST roster without replay", async () => {
    const { rest } = await urlDevice("A");
    select("A");
    const manager = terminalManager();
    const address = { deviceId: "A", entityId: "shell-checkout", kind: "terminals" };
    const scope = { run_id: address.entityId };
    await writeCached(address, { tabs: [] });
    const attaching = manager.attachTerminal("term-1", scope, {});
    const first = await openPeer();
    await answer(first, "term.attach", "attach_ok");
    await attaching;
    const creating = manager.createTerminal(scope, 80, 24).catch((error) => error);
    const closing = manager.closeTerminal("term-1", scope).catch((error) => error);
    await vi.waitFor(() => expect(first.take("term.close")).toHaveLength(1));
    expect(first.take("term.create")).toHaveLength(1);

    first.drop();
    expect(await Promise.all([creating, closing])).toEqual(Array(2).fill(expect.objectContaining({ uncertain: true })));
    await vi.waitFor(async () => expect((await readCached(address))?.value).toEqual({ tabs: [{ term_id: "term-1" }] }));
    expect(rest).toHaveBeenCalledWith("term.list", scope);
    const second = await openPeer(1);
    await answer(second, "term.attach", "attach_ok");
    expect(second.take("term.create")).toEqual([]);
    expect(second.take("term.close")).toEqual([]);
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
