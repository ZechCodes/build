// The terminal socket over the `term` DataChannel it is handed.
//
// It owns no socket: a session is minted for it through the followed device's
// rendezvous and adopted, a wire is handed over, and every open terminal
// re-attaches on it. Nothing here opens, reconnects or authenticates anything —
// that is core/rendezvous.js.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { TerminalSocket } from "../src/terminal/session.js";
import { createStatusHub } from "../src/terminal/statusHub.js";

const fakeTransport = {
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const b64 = (s) => btoa(s);
const dec = (bytes) => new TextDecoder().decode(bytes);

/** The session a rendezvous mints for the terminals of one device. */
const terminalSession = (deviceId = "dev-b", sessionId = "sess-term") => ({
  sessionId,
  sessionKeyB64: `key-${sessionId}`,
  deviceId,
});

/**
 * One `term` DataChannel, as its owner hands it over.
 *
 * Envelopes are recorded in the relay's own message shape so that what a test
 * reads off a wire reads the same whichever wire carried it.
 */
function fakeWire({ sendFails = null } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  const wire = {
    sent: [],
    closed: false,
    async send(envelope) {
      if (sendFails) throw new Error(sendFails);
      wire.sent.push({ type: "e2ee_envelope", envelope });
    },
    onEnvelope: subscribe(envelopeListeners),
    onClose: subscribe(closeListeners),
    close: () => {
      if (wire.closed) return;
      wire.closed = true;
      closeListeners.forEach((fn) => fn());
    },
    /** The bridge's half: one message down the channel. */
    serverSend: (message) => envelopeListeners.forEach((fn) => fn(message.envelope)),
  };
  return wire;
}

const lastPayload = (ws) => ws.sent.at(-1).envelope.frameFields.payload;
const respond = (ws, init, id, result) =>
  ws.serverSend({ type: "e2ee_envelope", envelope: { frameFields: { payload: { id, ok: true, result } } } });
const rejectCall = (ws, init, id, error) =>
  ws.serverSend({ type: "e2ee_envelope", envelope: { frameFields: { payload: { id, ok: false, error } } } });
const push = (ws, init, payload) =>
  ws.serverSend({ type: "e2ee_envelope", envelope: { frameFields: { payload } } });

/** Every request of one method sent on `ws`, in order. */
const callsOn = (ws, method) =>
  ws.sent
    .filter((m) => m.type === "e2ee_envelope" && m.envelope.frameFields.payload.method === method)
    .map((m) => m.envelope.frameFields.payload);

/** A socket with a session and a wire: what the manager hands over once the
 *  followed device's channels are open. */
async function connected({ session = terminalSession(), settle = tick } = {}) {
  const socket = new TerminalSocket({ transport: fakeTransport });
  const statuses = [];
  socket.onStatus((status) => statuses.push(status));
  socket.adoptTerminalSession(session);
  const ws = fakeWire();
  await socket.peer(ws);
  await settle();
  return { socket, ws, init: { session_id: session.sessionId }, statuses };
}

const connectedOnFakeTimers = () => connected({ settle: () => vi.advanceTimersByTimeAsync(0) });

/** The wire goes and another one takes its place — a peer connection that
 *  failed and came back on an ICE restart. The re-attach is left in flight for
 *  the caller to answer. */
function rewire(socket) {
  socket.peer(null);
  const next = fakeWire();
  socket.peer(next);
  return next;
}

describe("TerminalSocket", () => {
  it("attaches nothing when it is given a session and a wire (adopting is not attaching)", async () => {
    const { ws } = await connected();
    const methods = ws.sent.map((m) => m.envelope.frameFields.payload.method);
    expect(methods).not.toContain("term.attach");
  });

  it("is on the machine whose session it was given", async () => {
    const socket = new TerminalSocket({ transport: fakeTransport });
    expect(socket.deviceId).toBe(null);
    socket.adoptTerminalSession(terminalSession("dev-a"));
    expect(socket.deviceId).toBe("dev-a");
    socket.close();
  });

  it("says it is connecting while it has a session and no wire, and lost when told there is none", async () => {
    const socket = new TerminalSocket({ transport: fakeTransport });
    const statuses = [];
    socket.onStatus((status) => statuses.push(status));

    socket.adoptTerminalSession(terminalSession());
    expect(statuses).toEqual(["connecting"]);

    socket.peer(null); // the device it follows has no `term` channel
    expect(statuses).toEqual(["connecting", "disconnected"]);
    socket.close();
  });

  it("attachTerminal registers and sends term.attach {term_id, cols, rows}; output/reset dedupe per term", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const snapshots = [];
    const attaching = socket.attachTerminal("term-3", {
      cols: 100, rows: 30,
      onOutput: (b) => outputs.push(dec(b)),
      onSnapshot: (b) => snapshots.push(dec(b)),
    });
    attaching.catch(() => {});
    await tick();
    const attach = lastPayload(ws);
    expect(attach.method).toBe("term.attach");
    expect(attach.params).toEqual({ term_id: "term-3", cols: 100, rows: 30 });
    respond(ws, init, attach.id, { snapshot: b64("SCREEN"), cursor: 5 });
    await attaching;
    expect(snapshots).toEqual(["SCREEN"]);

    push(ws, init, { type: "term.output", term_id: "term-3", cursor: 6, data: b64("live") });
    push(ws, init, { type: "term.output", term_id: "term-3", cursor: 4, data: b64("stale") });
    push(ws, init, { type: "term.reset", term_id: "term-3", cursor: 8, data: b64("RESET") });
    // A frame for an id we never registered is ignored.
    push(ws, init, { type: "term.output", term_id: "term-99", cursor: 99, data: b64("ghost") });
    await tick();
    expect(outputs).toEqual(["live"]);
    expect(snapshots).toEqual(["SCREEN", "RESET"]);
    socket.close();
  });

  it("demuxes two terminals independently over one socket", async () => {
    const { socket, ws, init } = await connected();
    const a = [];
    const b = [];
    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onOutput: (x) => a.push(dec(x)), onSnapshot: () => {} });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;
    const at2 = socket.attachTerminal("term-2", { cols: 80, rows: 24, onOutput: (x) => b.push(dec(x)), onSnapshot: () => {} });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at2;

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 1, data: b64("one") });
    push(ws, init, { type: "term.output", term_id: "term-2", cursor: 1, data: b64("two") });
    await tick();
    expect(a).toEqual(["one"]);
    expect(b).toEqual(["two"]);
    socket.close();
  });

  // An agent belongs to a WORKTREE, and its wire id (`agent:<worktree_id>`) is a
  // hash the client cannot compute. So a surface attaches by what it holds — a
  // run/plan id, or the scope of the directory it is showing — and the bridge
  // answers with the id every later frame, keystroke and resize is keyed by.
  it("attachAgent sends the surface's own address and keys the screen by the id the bridge answers with", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const attaching = socket.attachAgent(
      { project_id: "p1", worktree_id: "wt-9" },
      { cols: 120, rows: 40, onSnapshot: () => {}, onOutput: (b) => outputs.push(dec(b)), onLive: () => {} },
    );
    attaching.catch(() => {});
    await tick();
    const attach = lastPayload(ws);
    expect(attach.method).toBe("agent.attach");
    expect(attach.params).toEqual({ project_id: "p1", worktree_id: "wt-9", cols: 120, rows: 40 });
    const result = await (respond(ws, init, attach.id, {
      term_id: "agent:wt-9",
      snapshot: b64(""),
      cursor: 0,
      live: true,
    }),
    attaching);
    // The caller learns the wire id from the attach — it is what term.input and
    // term.resize address, and what the pane must keep.
    expect(result.term_id).toBe("agent:wt-9");

    push(ws, init, { type: "term.output", term_id: "agent:wt-9", cursor: 1, data: b64("painting") });
    await tick();
    expect(outputs).toEqual(["painting"]);
    socket.close();
  });

  // The bridge registers this client under its state lock and enqueues the
  // response afterwards, so a pump flush can slip in between. Those bytes carry
  // the real wire id, which the client does not learn until the response lands —
  // dropping them would lose output permanently (the cursor only moves forward).
  it("keeps agent frames that outran the attach response, then replays them in order", async () => {
    const { socket, ws, init } = await connected();
    const snapshots = [];
    const outputs = [];
    const attaching = socket.attachAgent(
      { id: "run-3" },
      {
        cols: 120, rows: 40,
        onSnapshot: (b) => snapshots.push(dec(b)),
        onOutput: (b) => outputs.push(dec(b)),
        onLive: () => {},
      },
    );
    attaching.catch(() => {});
    await tick();
    const attach = lastPayload(ws);

    // Bytes for an id this client cannot yet have registered.
    push(ws, init, { type: "term.output", term_id: "agent:wt-7", cursor: 6, data: b64("outran") });
    await tick();
    respond(ws, init, attach.id, { term_id: "agent:wt-7", snapshot: b64("SCREEN"), cursor: 5, live: true });
    await attaching;
    await tick();
    expect(snapshots).toEqual(["SCREEN"]);
    expect(outputs).toEqual(["outran"]);
    socket.close();
  });

  it("term.closed deregisters a user terminal but retains an agent screen on session-end", async () => {
    const { socket, ws, init } = await connected();
    const userClosed = [];
    const agentClosed = [];
    const agentOut = [];

    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: () => {}, onClosed: (r) => userClosed.push(r) });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;

    const at2 = socket.attachAgent({ id: "task-5" }, {
      cols: 120, rows: 40,
      onSnapshot: () => {}, onOutput: (x) => agentOut.push(dec(x)),
      onClosed: (r) => agentClosed.push(r), onLive: () => {},
    });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-5", snapshot: b64(""), cursor: 0, live: true });
    await at2;

    // User terminal exits → onClosed + deregister (a later frame is ignored).
    push(ws, init, { type: "term.closed", term_id: "term-1", reason: "exited" });
    await tick();
    expect(userClosed).toEqual(["exited"]);
    const outsBefore = agentOut.length;
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 9, data: b64("zombie") });
    await tick();

    // Agent session ends → onClosed fires but the screen stays registered.
    push(ws, init, { type: "term.closed", term_id: "agent:wt-5", reason: "agent_session_ended" });
    await tick();
    expect(agentClosed).toEqual(["agent_session_ended"]);
    // A new session's output still routes to the retained agent screen.
    push(ws, init, { type: "term.output", term_id: "agent:wt-5", cursor: 1, data: b64("next") });
    await tick();
    expect(agentOut.slice(outsBefore)).toEqual(["next"]);
    socket.close();
  });

  it("reconnect re-attaches every registered terminal and resets cursors from the new snapshots", async () => {
    const { socket, ws, init } = await connected();
    const userSnaps = [];
    const agentSnaps = [];
    const userClosed = [];
    const lives = [];

    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: (b) => userSnaps.push(dec(b)), onClosed: (r) => userClosed.push(r) });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64("U1"), cursor: 3 });
    await at1;

    const at2 = socket.attachAgent({ id: "task-5" }, { cols: 120, rows: 40, onSnapshot: (b) => agentSnaps.push(dec(b)), onLive: (l) => lives.push(l) });
    at2.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-5", snapshot: b64("A1"), cursor: 10, live: true });
    await at2;
    expect(lives).toEqual([true]);

    // Drop → the socket reconnects with backoff and re-attaches every term.
    // The channel goes and an ICE restart brings another: every term re-attaches.
    const ws2 = rewire(socket);
    await tick();
    expect(ws2).not.toBe(ws);
    const init2 = init;

    // First re-attach (user term-1) → rejected as unknown → reaped + deregistered.
    let p = lastPayload(ws2);
    expect(p.method).toBe("term.attach");
    expect(p.params).toEqual({ term_id: "term-1", cols: 80, rows: 24 });
    rejectCall(ws2, init2, p.id, "unknown term_id");
    await tick();
    expect(userClosed).toEqual(["reaped"]);

    // Then the agent re-attaches via agent.attach and updates onLive.
    await tick();
    p = lastPayload(ws2);
    expect(p.method).toBe("agent.attach");
    expect(p.params).toEqual({ id: "task-5", cols: 120, rows: 40 });
    respond(ws2, init2, p.id, { term_id: "agent:wt-5", snapshot: b64("A2"), cursor: 20, live: false });
    await tick();
    expect(agentSnaps).toEqual(["A1", "A2"]);
    expect(lives).toEqual([true, false]);
    socket.close();
  });

  it("applies an equal-cursor term.reset (the agent pump's start-of-session wipe)", async () => {
    const { socket, ws, init } = await connected();
    const snaps = [];
    const outputs = [];
    // Attach to a task whose previous session's screen is retained: the response
    // carries cursor == total. The pump the attach spawned then wipes with
    // term.reset{cursor: total} — resets never advance `total`, so the cursor is
    // EQUAL to the attach cursor. The wipe must still apply or the new session's
    // bytes garble over the old screen.
    const at = socket.attachAgent({ id: "task-9" }, {
      cols: 120, rows: 40,
      onSnapshot: (b) => snaps.push(dec(b)),
      onOutput: (b) => outputs.push(dec(b)),
      onLive: () => {},
    });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-9", snapshot: b64("OLD SCREEN"), cursor: 42, live: true });
    await at;
    expect(snaps).toEqual(["OLD SCREEN"]);

    push(ws, init, { type: "term.reset", term_id: "agent:wt-9", cursor: 42, data: b64("") });
    push(ws, init, { type: "term.output", term_id: "agent:wt-9", cursor: 50, data: b64("new session") });
    await tick();
    expect(snaps).toEqual(["OLD SCREEN", ""]);
    expect(outputs).toEqual(["new session"]);
    socket.close();
  });

  // A worktree with no agent yet can only be addressed by the worktree, so the
  // attach answers `agent:<worktree_id>` — a placeholder. The agent is born
  // later, and its frames carry `agent:<agent_id>`: a different id, on the very
  // screen this client is looking at. The opening term.reset is what names it.
  it("re-keys a waiting agent screen onto the id the newborn agent's reset carries", async () => {
    const { socket, ws, init } = await connected();
    const snaps = [];
    const outputs = [];
    const lives = [];
    const ids = [];
    const at = socket.attachAgent({ project_id: "p1", worktree_id: "wt-2" }, {
      cols: 120, rows: 40,
      onSnapshot: (b) => snaps.push(dec(b)),
      onOutput: (b) => outputs.push(dec(b)),
      onLive: (l) => lives.push(l),
      onTermId: (id) => ids.push(id),
    });
    at.catch(() => {});
    await tick();
    // No agent has ever run here: no provider, not live, blank screen.
    respond(ws, init, lastPayload(ws).id, {
      term_id: "agent:wt-2", snapshot: b64(""), cursor: 0, live: false, provider: null,
    });
    await at;
    expect(ids).toEqual(["agent:wt-2"]);

    // The first message spawns the agent; the pump wipes the screen it is born
    // onto and starts painting — under the AGENT's id.
    push(ws, init, { type: "term.reset", term_id: "agent:ag-77", cursor: 0, data: b64("") });
    push(ws, init, { type: "term.output", term_id: "agent:ag-77", cursor: 3, data: b64("painting") });
    await tick();
    // The attach's blank screen, then the newborn session's own wipe.
    expect(snaps).toEqual(["", ""]);
    expect(outputs).toEqual(["painting"]);
    expect(lives).toEqual([false, true]);
    // The caller is told the id its keystrokes must now address.
    expect(ids).toEqual(["agent:wt-2", "agent:ag-77"]);
    // …and the screen's own ack names it too.
    await vi.waitFor(() => {
      const ack = ws.sent
        .filter((m) => m.type === "e2ee_envelope")
        .map((m) => m.envelope.frameFields.payload)
        .find((p) => p.method === "term.ack");
      expect(ack.params).toEqual({ term_id: "agent:ag-77", cursor: 3 });
    });
    socket.close();
  });

  // A retained screen already names its agent, so a new session on it is not a
  // birth: its frames arrive under the id the attach gave, and nothing re-keys.
  it("never re-keys an agent screen that already named its agent", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const at = socket.attachAgent({ id: "run-8" }, {
      cols: 120, rows: 40,
      onSnapshot: () => {}, onOutput: (b) => outputs.push(dec(b)), onLive: () => {},
    });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, {
      term_id: "agent:ag-1", snapshot: b64("LAST"), cursor: 5, live: false, provider: "claude",
    });
    await at;

    // Another agent's screen is not this one: its frames belong to a bubble
    // this client is not showing, and must not land here.
    push(ws, init, { type: "term.reset", term_id: "agent:ag-2", cursor: 0, data: b64("") });
    push(ws, init, { type: "term.output", term_id: "agent:ag-2", cursor: 1, data: b64("elsewhere") });
    await tick();
    expect(outputs).toEqual([]);
    socket.close();
  });

  it("a streamed frame on an idle agent screen reports the session live again", async () => {
    const { socket, ws, init } = await connected();
    const lives = [];
    const closed = [];
    const at = socket.attachAgent({ id: "task-4" }, {
      cols: 120, rows: 40,
      onSnapshot: () => {}, onOutput: () => {},
      onLive: (l) => lives.push(l), onClosed: (r) => closed.push(r),
    });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-4", snapshot: b64("LAST"), cursor: 10, live: false });
    await at;
    expect(lives).toEqual([false]);

    // A new session starts while the tab stays mounted: the pump's reset +
    // output arrive with no new attach. The screen must report live again so
    // the "no active agent session" chip clears.
    push(ws, init, { type: "term.reset", term_id: "agent:wt-4", cursor: 10, data: b64("") });
    await tick();
    expect(lives).toEqual([false, true]);
    // Further frames do not re-report (no chip-toggling spam).
    push(ws, init, { type: "term.output", term_id: "agent:wt-4", cursor: 12, data: b64("x") });
    await tick();
    expect(lives).toEqual([false, true]);

    // Session ends (screen retained) → the NEXT session's frames re-report live.
    push(ws, init, { type: "term.closed", term_id: "agent:wt-4", reason: "agent_session_ended" });
    push(ws, init, { type: "term.output", term_id: "agent:wt-4", cursor: 20, data: b64("y") });
    await tick();
    expect(closed).toEqual(["agent_session_ended"]);
    expect(lives).toEqual([false, true, true]);
    socket.close();
  });

  it("buffers pushes that outrun the attach response and replays them after the snapshot", async () => {
    const { socket, ws, init } = await connected();
    const order = [];
    const at = socket.attachTerminal("term-5", {
      cols: 80, rows: 24,
      onSnapshot: (b) => order.push(`snap:${dec(b)}`),
      onOutput: (b) => order.push(`out:${dec(b)}`),
    });
    at.catch(() => {});
    await tick();
    const attach = lastPayload(ws);
    // The bridge registers the sender under the AppState lock, but the response
    // is enqueued after the handler returns — a 10ms pump flush can slip in
    // ahead of it. Those bytes are PAST the snapshot cursor and must not be
    // wiped by the late-applied snapshot.
    push(ws, init, { type: "term.output", term_id: "term-5", cursor: 4, data: b64("stale") });
    push(ws, init, { type: "term.output", term_id: "term-5", cursor: 7, data: b64("tail") });
    respond(ws, init, attach.id, { snapshot: b64("SNAP"), cursor: 5 });
    await at;
    expect(order).toEqual(["snap:SNAP", "out:tail"]);
    socket.close();
  });

  it("a rejected attach deregisters the terminal instead of leaking a dead entry", async () => {
    const { socket, ws, init } = await connected();
    const outputs = [];
    const at = socket.attachTerminal("term-8", { cols: 80, rows: 24, onOutput: (b) => outputs.push(dec(b)), onSnapshot: () => {} });
    await tick();
    rejectCall(ws, init, lastPayload(ws).id, "unknown term_id");
    await expect(at).rejects.toThrow(/unknown term_id/);

    // The dead id is gone: pushes are ignored and a reconnect does not retry it.
    push(ws, init, { type: "term.output", term_id: "term-8", cursor: 3, data: b64("ghost") });
    await tick();
    expect(outputs).toEqual([]);
    const ws2 = rewire(socket);
    await tick();
    const reattaches = ws2.sent
      .filter((m) => m.type === "e2ee_envelope")
      .map((m) => m.envelope.frameFields.payload.method)
      .filter((m) => m === "term.attach" || m === "agent.attach");
    expect(reattaches).toEqual([]);
    socket.close();
  });

  it("reconnect reaps an agent screen whose task is gone (no eternal retry)", async () => {
    const { socket, ws, init } = await connected();
    const closed = [];
    const at = socket.attachAgent({ id: "task-7" }, { cols: 120, rows: 40, onSnapshot: () => {}, onLive: () => {}, onClosed: (r) => closed.push(r) });
    at.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-7", snapshot: b64("A"), cursor: 1, live: false });
    await at;

    // The task is deleted while we are away; the re-attach is rejected.
    const ws2 = rewire(socket);
    await tick();
    const init2 = init;
    const p = lastPayload(ws2);
    expect(p.method).toBe("agent.attach");
    rejectCall(ws2, init2, p.id, "unknown id");
    await tick();
    expect(closed).toEqual(["reaped"]);

    // Deregistered: the next reconnect does not retry it.
    const ws3 = rewire(socket);
    await tick();
    const retries = ws3.sent
      .filter((m) => m.type === "e2ee_envelope")
      .map((m) => m.envelope.frameFields.payload.method)
      .filter((m) => m === "agent.attach");
    expect(retries).toEqual([]);
    socket.close();
  });

  it("create/list/close carry the scope spread and the right shapes", async () => {
    const { socket, ws, init } = await connected();

    // term.create asks for a scope and a grid, and nothing else: every terminal
    // the human opens is their shell. The one agent of a worktree is Build's,
    // and it is started by a delivery into its own tab — never from here.
    const creating = socket.createTerminal({ project_id: "proj-1" }, 80, 24);
    creating.catch(() => {});
    await tick();
    let p = lastPayload(ws);
    expect(p.method).toBe("term.create");
    expect(p.params).toEqual({ project_id: "proj-1", cols: 80, rows: 24 });
    respond(ws, init, p.id, { term_id: "term-7", kind: "shell", cols: 80, rows: 24 });
    expect(await creating).toEqual({ term_id: "term-7", kind: "shell", cols: 80, rows: 24 });

    const listing = socket.listTerminals({ run_id: "run-3" });
    listing.catch(() => {});
    await tick();
    p = lastPayload(ws);
    expect(p.method).toBe("term.list");
    expect(p.params).toEqual({ run_id: "run-3" });
    respond(ws, init, p.id, { terminals: [{ term_id: "term-1", cols: 80, rows: 24 }] });
    expect(await listing).toEqual([{ term_id: "term-1", cols: 80, rows: 24 }]);

    // Register then close: the entry deregisters even though the server round-trips.
    const at1 = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: () => {} });
    at1.catch(() => {});
    await tick();
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await at1;
    const closing = socket.closeTerminal("term-1");
    closing.catch(() => {});
    await tick();
    p = lastPayload(ws);
    expect(p.method).toBe("term.close");
    expect(p.params).toEqual({ term_id: "term-1" });
    respond(ws, init, p.id, { ok: true });
    await closing;
    // A push to the now-closed id is ignored.
    const out = [];
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 5, data: b64("x") });
    await tick();
    expect(out).toEqual([]);
    socket.close();
  });

  it("keeps a user terminal's workspace scope when reconnecting", async () => {
    const { socket, ws, init } = await connected();
    const attaching = socket.attachTerminal(
      "term-4",
      { workspace_id: "ws-7", source_id: "src-2" },
      { cols: 90, rows: 30, onSnapshot: () => {} },
    );
    attaching.catch(() => {});
    await tick();
    let p = lastPayload(ws);
    expect(p.params).toEqual({ workspace_id: "ws-7", term_id: "term-4", cols: 90, rows: 30 });
    respond(ws, init, p.id, { snapshot: b64(""), cursor: 0 });
    await attaching;

    const ws2 = rewire(socket);
    await tick();
    const init2 = init;
    p = lastPayload(ws2);
    expect(p.method).toBe("term.attach");
    expect(p.params).toEqual({ workspace_id: "ws-7", term_id: "term-4", cols: 90, rows: 30 });
    respond(ws2, init2, p.id, { snapshot: b64(""), cursor: 0 });
    await tick();
    socket.close();
  });
});

// The liveness ping shares ONE FIFO with terminal output, so a flooding PTY
// delays the pong. These tests pin the rule that decides between "busy" and
// "dead": received frames vouch for the connection, silence does not.
describe("TerminalSocket liveness", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const pingsSentOn = (ws) => callsOn(ws, "ping");

  it("stays connected while output keeps arriving, even though no ping is ever answered", async () => {
    const { socket, ws, init, statuses } = await connectedOnFakeTimers();
    const attaching = socket.attachTerminal("term-flood", { onOutput: () => {}, onSnapshot: () => {} });
    attaching.catch(() => {});
    await vi.advanceTimersByTimeAsync(0);
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await attaching;
    statuses.length = 0;

    // 20s of steady PTY output and not one pong: the bridge is busy, not dead.
    for (let cursor = 1; cursor <= 20; cursor++) {
      push(ws, init, { type: "term.output", term_id: "term-flood", cursor, data: b64("x") });
      await vi.advanceTimersByTimeAsync(1000);
    }
    expect(pingsSentOn(ws)).toEqual([]); // data made the probe unnecessary
    expect(statuses).toEqual([]); // no false disconnect, no reconnect loop
    socket.close();
  });

  // The wire that did not answer is the one that goes: closing the channel is
  // how it reports itself gone, and its owner decides what that costs — an ICE
  // restart, or the device blocked. Nothing here reconnects anything.
  it("closes an idle wire whose ping goes unanswered and says the shells are lost", async () => {
    const { socket, ws, statuses } = await connectedOnFakeTimers();
    // Its owner is what hands the channel over and takes it back, exactly as
    // terminal/manager.js does for the peer link's `term` half.
    ws.onClose(() => socket.peer(null));
    statuses.length = 0;

    await vi.advanceTimersByTimeAsync(2000);
    expect(pingsSentOn(ws).length).toBe(1); // silence → probe
    await vi.advanceTimersByTimeAsync(3000); // the ping's own timeout

    expect(ws.closed).toBe(true);
    expect(statuses).toEqual(["disconnected"]);
    socket.close();
  });

  it("stops the liveness loop after close()", async () => {
    const { socket, ws, statuses } = await connectedOnFakeTimers();
    statuses.length = 0;
    socket.close();
    await vi.advanceTimersByTimeAsync(20000);
    expect(pingsSentOn(ws)).toEqual([]);
    expect(statuses).toEqual([]);
  });

  it("stops the liveness loop of a superseded wire (no stray disconnect)", async () => {
    const { socket, ws, statuses } = await connectedOnFakeTimers();
    statuses.length = 0;

    socket.peer(null);
    await vi.advanceTimersByTimeAsync(0);
    expect(statuses).toEqual(["disconnected"]);

    // Past the next liveness wake: the old wire's loop must be gone, so nothing
    // is probed and nothing further is said about a socket with no wire.
    await vi.advanceTimersByTimeAsync(2500);
    expect(pingsSentOn(ws)).toEqual([]);
    expect(statuses).toEqual(["disconnected"]);
    socket.close();
  });
});

// The bridge cannot see this browser's receive queue, so it streams blind until
// the client tells it how far it has actually got. These tests pin what a
// terminal owes the bridge — one cheap, advisory cursor report — and the two
// things it must never do with it: cost a call per frame, or be worth a
// disconnect.
describe("TerminalSocket output acks", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const acksOn = (ws) => callsOn(ws, "term.ack");

  /** An attached user terminal on fake timers, with the socket's connection. */
  async function attachedOnFakeTimers(termId, cursor = 0) {
    const ctx = await connectedOnFakeTimers();
    const attaching = ctx.socket.attachTerminal(termId, { onOutput: () => {}, onSnapshot: () => {} });
    attaching.catch(() => {});
    await vi.advanceTimersByTimeAsync(0);
    respond(ctx.ws, ctx.init, lastPayload(ctx.ws).id, { snapshot: b64(""), cursor });
    await attaching;
    return ctx;
  }

  it("reports one cursor per throttle window, carrying the latest one applied", async () => {
    const { socket, ws, init } = await attachedOnFakeTimers("term-1");

    // A burst: ten coalesced frames land inside one window.
    for (let cursor = 1; cursor <= 10; cursor++) {
      push(ws, init, { type: "term.output", term_id: "term-1", cursor, data: b64("x") });
    }
    await vi.advanceTimersByTimeAsync(0);
    expect(acksOn(ws)).toEqual([]); // a frame does not cost a call

    await vi.advanceTimersByTimeAsync(300);
    let acks = acksOn(ws);
    expect(acks.length).toBe(1);
    expect(acks[0].params).toEqual({ term_id: "term-1", cursor: 10 });

    // The next window reports the next burst — a reset counts as applied too.
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 11, data: b64("y") });
    push(ws, init, { type: "term.reset", term_id: "term-1", cursor: 20, data: b64("SCREEN") });
    await vi.advanceTimersByTimeAsync(300);
    acks = acksOn(ws);
    expect(acks.length).toBe(2);
    expect(acks[1].params).toEqual({ term_id: "term-1", cursor: 20 });

    // A frame the terminal did NOT apply (stale cursor) is not worth an ack.
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 3, data: b64("stale") });
    await vi.advanceTimersByTimeAsync(300);
    expect(acksOn(ws).length).toBe(2);
    socket.close();
  });

  it("swallows a rejected ack — telemetry is never worth a disconnect", async () => {
    const { socket, ws, init, statuses } = await attachedOnFakeTimers("term-1");
    const outputs = [];
    socket.detach("term-1");
    const reattaching = socket.attachTerminal("term-1", { onOutput: (b) => outputs.push(dec(b)), onSnapshot: () => {} });
    reattaching.catch(() => {});
    await vi.advanceTimersByTimeAsync(0);
    respond(ws, init, lastPayload(ws).id, { snapshot: b64(""), cursor: 0 });
    await reattaching;
    statuses.length = 0;

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 1, data: b64("a") });
    await vi.advanceTimersByTimeAsync(300);
    const ack = acksOn(ws).at(-1);
    rejectCall(ws, init, ack.id, "unknown term_id");
    await vi.advanceTimersByTimeAsync(0);

    expect(statuses).toEqual([]); // no teardown, no reconnect loop
    // …and the terminal is still streaming.
    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 2, data: b64("b") });
    await vi.advanceTimersByTimeAsync(300);
    expect(outputs).toEqual(["a", "b"]);
    socket.close();
  });

  it("sends no ack for a terminal the tab detached", async () => {
    const { socket, ws, init } = await attachedOnFakeTimers("term-1");

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 7, data: b64("x") });
    await vi.advanceTimersByTimeAsync(0);
    socket.detach("term-1"); // unmounted inside the throttle window

    await vi.advanceTimersByTimeAsync(1000);
    expect(acksOn(ws)).toEqual([]);
    socket.close();
  });

  it("sends no ack after close() — a closed socket owes the bridge nothing", async () => {
    const { socket, ws, init } = await attachedOnFakeTimers("term-1");

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 7, data: b64("x") });
    await vi.advanceTimersByTimeAsync(0);
    socket.close();

    await vi.advanceTimersByTimeAsync(20000);
    expect(acksOn(ws)).toEqual([]);
  });

  it("drops a pending ack when the connection does — a cursor never crosses connections", async () => {
    const { socket, ws, init } = await attachedOnFakeTimers("term-1");

    push(ws, init, { type: "term.output", term_id: "term-1", cursor: 9, data: b64("x") });
    await vi.advanceTimersByTimeAsync(0);
    const ws2 = rewire(socket); // the ack is still inside its window
    await vi.advanceTimersByTimeAsync(0);

    await vi.advanceTimersByTimeAsync(600); // past the ack's throttle window
    expect(ws2).not.toBe(ws);
    expect(acksOn(ws)).toEqual([]); // nothing chased the dead wire
    const init2 = init;
    // The re-attach rebases the cursor far past the one that was pending.
    respond(ws2, init2, lastPayload(ws2).id, { snapshot: b64(""), cursor: 500 });
    await vi.advanceTimersByTimeAsync(1000);
    expect(acksOn(ws2)).toEqual([]); // the stale timer did not survive the reconnect

    // The new connection acks its own frames normally.
    push(ws2, init2, { type: "term.output", term_id: "term-1", cursor: 501, data: b64("y") });
    await vi.advanceTimersByTimeAsync(300);
    expect(acksOn(ws2).map((a) => a.params)).toEqual([{ term_id: "term-1", cursor: 501 }]);
    socket.close();
  });

  it("acks an agent screen under the wire id the bridge named, not the provisional one", async () => {
    const { socket, ws, init } = await connectedOnFakeTimers();
    const attaching = socket.attachAgent({ id: "run-3" }, { onSnapshot: () => {}, onOutput: () => {}, onLive: () => {} });
    attaching.catch(() => {});
    await vi.advanceTimersByTimeAsync(0);
    respond(ws, init, lastPayload(ws).id, { term_id: "agent:wt-3", snapshot: b64(""), cursor: 0, live: true });
    await attaching;

    push(ws, init, { type: "term.output", term_id: "agent:wt-3", cursor: 4, data: b64("painting") });
    await vi.advanceTimersByTimeAsync(300);
    expect(acksOn(ws).map((a) => a.params)).toEqual([{ term_id: "agent:wt-3", cursor: 4 }]);
    socket.close();
  });
});

// A caller that asks the socket for something before it is up waits on
// whenConnected(). A wait that only ever RESOLVES is the wedge amplifier: when
// the socket dies, every waiter hangs forever, the surfaces that asked never
// finish, and their retries stack behind a socket that is not coming back on
// its own. Every wait ends — with a connection, or with a typed loss.
describe("a caller waiting on a wire that never came", () => {
  /** A socket with a session and no wire yet: what the manager holds between
   *  minting a terminal session and the device's channels opening. */
  function waitingForAWire() {
    const socket = new TerminalSocket({ transport: fakeTransport });
    const statuses = [];
    socket.onStatus((status) => statuses.push(status));
    socket.adoptTerminalSession(terminalSession());
    return { socket, statuses };
  }

  it("rejects pending whenConnected waiters with a typed loss instead of hanging", async () => {
    const { socket } = waitingForAWire();
    const waiting = socket.whenConnected();
    socket.peer(null); // the device it follows cannot carry them
    await expect(waiting).rejects.toMatchObject({ name: "TerminalSocketLost", reason: "disconnected" });
    socket.close();
  });

  it("rejects the calls that were waiting on it, so nothing stacks behind a dead wire", async () => {
    const { socket } = waitingForAWire();
    const attaching = socket.attachTerminal("term-1", { cols: 80, rows: 24, onSnapshot: () => {} });
    const listing = socket.listTerminals({ run_id: "run-3" });
    const creating = socket.createTerminal({ run_id: "run-3" }, 80, 24);
    socket.peer(null);
    await expect(attaching).rejects.toMatchObject({ name: "TerminalSocketLost" });
    await expect(listing).rejects.toMatchObject({ name: "TerminalSocketLost" });
    await expect(creating).rejects.toMatchObject({ name: "TerminalSocketLost" });
    socket.close();
  });

  it("rejects again on the next wire that does not come, rather than swallowing a later waiter", async () => {
    const { socket } = waitingForAWire();
    socket.peer(null);
    await tick();
    const waiting = socket.whenConnected(); // asked while the upgrade is in flight
    socket.peer(null); // …and that attempt failed too
    await expect(waiting).rejects.toMatchObject({ name: "TerminalSocketLost", reason: "disconnected" });
    socket.close();
  });

  it("rejects waiters on close(), and every wait asked of a closed socket", async () => {
    const { socket } = waitingForAWire();
    const waiting = socket.whenConnected();
    socket.close();
    await expect(waiting).rejects.toMatchObject({ name: "TerminalSocketLost", reason: "closed" });
    await expect(socket.whenConnected()).rejects.toMatchObject({ reason: "closed" });
  });

  it("resolves the wait the moment a wire carries, with every terminal already attached", async () => {
    const { socket, statuses } = waitingForAWire();
    const waiting = socket.whenConnected();
    const ws = fakeWire();
    await socket.peer(ws);
    await expect(waiting).resolves.toBeUndefined();
    expect(statuses).toEqual(["connecting", "connected"]);
    socket.close();
  });
});

describe("createStatusHub (per-pane connectivity fan-out)", () => {
  it("delivers the current status to a new subscriber, but only once a status is known", () => {
    const hub = createStatusHub();
    const early = [];
    hub.subscribe((s) => early.push(s)); // nothing set yet — nothing delivered
    expect(early).toEqual([]);

    hub.set("connected");
    expect(early).toEqual(["connected"]);

    const late = [];
    hub.subscribe((s) => late.push(s)); // a late subscriber gets the current status now
    expect(late).toEqual(["connected"]);
  });

  it("fans a status change out to every subscriber", () => {
    const hub = createStatusHub();
    const a = [];
    const b = [];
    hub.subscribe((s) => a.push(s));
    hub.subscribe((s) => b.push(s));
    hub.set("disconnected");
    expect(a).toEqual(["disconnected"]);
    expect(b).toEqual(["disconnected"]);
  });

  it("stops delivering after unsubscribe", () => {
    const hub = createStatusHub();
    const seen = [];
    const unsubscribe = hub.subscribe((s) => seen.push(s));
    hub.set("connecting");
    unsubscribe();
    hub.set("connected");
    expect(seen).toEqual(["connecting"]);
  });
});
