// One wire, whichever wire it is. Nothing above a carrier may learn which
// implementation it holds, so both are exercised through the same four verbs.

import { describe, it, expect, vi } from "vitest";
import { openCarrier, DC_BUFFERED_HIGH } from "../src/core/carrier.js";
import { CHUNK_BYTES, createReassembler, splitEnvelope } from "../src/core/chunk.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

class FakeEventTarget {
  constructor() {
    this.listeners = {};
  }
  addEventListener(type, fn) {
    (this.listeners[type] ||= []).push(fn);
  }
  emit(type, event = {}) {
    (this.listeners[type] || []).forEach((fn) => fn(event));
  }
}

// A socket with a lifecycle, because the bug this fake used to hide is a write
// to one that has ended: a browser throws "WebSocket is already in CLOSING or
// CLOSED state" there, and a fake that took the write happily could not say so.
const SOCKET_OPEN = 1;
const SOCKET_CLOSED = 3;

class FakeSocket extends FakeEventTarget {
  constructor() {
    super();
    this.sent = [];
    this.closed = false;
    this.readyState = SOCKET_OPEN;
  }
  send(text) {
    if (this.readyState !== SOCKET_OPEN) throw new Error("WebSocket is already in CLOSING or CLOSED state.");
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.closed = true;
    this.readyState = SOCKET_CLOSED;
    this.emit("close");
  }
}

class FakeChannel extends FakeEventTarget {
  constructor() {
    super();
    this.sent = [];
    this.readyState = "open";
    this.bufferedAmount = 0;
    this.bufferedAmountLowThreshold = 0;
  }
  send(text) {
    this.sent.push(text);
    this.bufferedAmount += text.length;
  }
  close() {
    this.readyState = "closed";
    this.emit("close");
  }
  deliver(text) {
    this.emit("message", { data: text });
  }
  drainTo(bytes) {
    this.bufferedAmount = bytes;
    this.emit("bufferedamountlow");
  }
}

const envelope = { version: 1, session_id: "sess-1", route_to: "device:d", nonce: "n", ciphertext: "c" };

describe("the relay carrier", () => {
  it("wraps what it sends in the relay's envelope message", () => {
    const socket = new FakeSocket();
    openCarrier({ socket, sessionId: "sess-1" }).send(envelope);
    expect(socket.sent).toEqual([{ type: "e2ee_envelope", session_id: "sess-1", envelope }]);
  });

  it("hands up envelopes and ignores the relay's own control messages", () => {
    const socket = new FakeSocket();
    const seen = [];
    openCarrier({ socket, sessionId: "sess-1" }).onEnvelope((e) => seen.push(e));
    socket.emit("message", { data: JSON.stringify({ type: "device_offline", device_id: "d" }) });
    socket.emit("message", { data: JSON.stringify({ type: "e2ee_envelope", session_id: "sess-1", envelope }) });
    expect(seen).toEqual([envelope]);
  });

  it("takes only its own session's envelopes off a socket that carries several", () => {
    const socket = new FakeSocket();
    const mine = [];
    const theirs = [];
    openCarrier({ socket, sessionId: "sess-1" }).onEnvelope((e) => mine.push(e));
    openCarrier({ socket, sessionId: "sess-2" }).onEnvelope((e) => theirs.push(e));
    socket.emit("message", { data: JSON.stringify({ type: "e2ee_envelope", session_id: "sess-2", envelope }) });
    expect(mine).toEqual([]);
    expect(theirs).toEqual([envelope]);
  });

  it("reports the wire gone once, whether the socket dropped or we let it go", () => {
    const socket = new FakeSocket();
    const carrier = openCarrier({ socket, sessionId: "sess-1" });
    const closed = vi.fn();
    carrier.onClose(closed);
    carrier.close();
    socket.emit("close");
    expect(closed).toHaveBeenCalledTimes(1);
  });

  // The socket belongs to the rendezvous that opened it, and one rendezvous
  // mints several sessions on it (spec rule 5). A session letting its signaling
  // carrier go must not take the wire out from under the others.
  it("leaves the socket to whoever else is riding it", () => {
    const socket = new FakeSocket();
    const mine = openCarrier({ socket, sessionId: "sess-1" });
    const theirs = openCarrier({ socket, sessionId: "sess-2" });
    const stillThere = [];
    theirs.onEnvelope((e) => stillThere.push(e));

    mine.close();

    expect(socket.closed).toBe(false);
    socket.emit("message", { data: JSON.stringify({ type: "e2ee_envelope", session_id: "sess-2", envelope }) });
    expect(stillThere).toEqual([envelope]);
  });

  it("stops delivering to a carrier that has been let go", () => {
    const socket = new FakeSocket();
    const carrier = openCarrier({ socket, sessionId: "sess-1" });
    const seen = [];
    carrier.onEnvelope((e) => seen.push(e));

    carrier.close();
    socket.emit("message", { data: JSON.stringify({ type: "e2ee_envelope", session_id: "sess-1", envelope }) });

    expect(seen).toEqual([]);
  });
});

describe("the DataChannel carrier", () => {
  it("sends an envelope that fits as one message", async () => {
    const channel = new FakeChannel();
    await openCarrier({ channel }).send(envelope);
    expect(channel.sent).toEqual([JSON.stringify(envelope)]);
  });

  it("sends an oversized envelope as parts the bridge's reassembler reads", async () => {
    const channel = new FakeChannel();
    const big = { ...envelope, ciphertext: "c".repeat(CHUNK_BYTES * 2) };
    await openCarrier({ channel }).send(big);
    expect(channel.sent.length).toBeGreaterThan(1);
    const reassembler = createReassembler();
    const reassembled = channel.sent.map((part) => reassembler.accept(part)).at(-1);
    expect(JSON.parse(reassembled)).toEqual(big);
  });

  it("hands up an envelope reassembled from its parts", async () => {
    const channel = new FakeChannel();
    const seen = [];
    openCarrier({ channel }).onEnvelope((e) => seen.push(e));
    const big = { ...envelope, ciphertext: "c".repeat(CHUNK_BYTES * 2) };
    for (const part of splitEnvelope(JSON.stringify(big))) channel.deliver(part);
    expect(seen).toEqual([big]);
  });

  it("closes the channel on a reassembly it cannot finish", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    const closed = vi.fn();
    carrier.onClose(closed);
    const parts = splitEnvelope("x".repeat(CHUNK_BYTES * 3));
    channel.deliver(parts[0]);
    channel.deliver(parts[2]);
    expect(channel.readyState).toBe("closed");
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("reports the wire gone once when the channel closes", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    const closed = vi.fn();
    carrier.onClose(closed);
    channel.close();
    carrier.close();
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("holds the writer at the buffer limit and wakes when the peer drains", async () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    channel.bufferedAmount = DC_BUFFERED_HIGH + 1;
    const parked = carrier.send(envelope);
    let settled = false;
    parked.then(() => (settled = true));
    await tick();
    expect(channel.sent).toEqual([]);
    expect(settled).toBe(false);
    channel.drainTo(0);
    await parked;
    expect(channel.sent).toEqual([JSON.stringify(envelope)]);
  });

  it("fails a send on a channel that is gone rather than queueing for nobody", async () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    channel.close();
    await expect(carrier.send(envelope)).rejects.toThrow();
  });

  it("wakes a parked writer when the channel dies under it", async () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    channel.bufferedAmount = DC_BUFFERED_HIGH + 1;
    const parked = carrier.send(envelope);
    await tick();
    channel.close();
    await expect(parked).rejects.toThrow();
  });
});

describe("a carrier's listeners", () => {
  it("delivers to every envelope listener, and to none that unsubscribed", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    const first = [];
    const second = [];
    carrier.onEnvelope((e) => first.push(e));
    const stopSecond = carrier.onEnvelope((e) => second.push(e));
    channel.deliver(JSON.stringify(envelope));
    stopSecond();
    channel.deliver(JSON.stringify(envelope));
    expect(first).toHaveLength(2);
    expect(second).toHaveLength(1);
  });

  it("reports the wire gone to every close listener, not only the last one", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    const owners = [vi.fn(), vi.fn()];
    for (const owner of owners) carrier.onClose(owner);
    channel.close();
    for (const owner of owners) expect(owner).toHaveBeenCalledTimes(1);
  });

  it("closes the channel on a reassembly that completes into something that is not an envelope", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    const closed = vi.fn();
    carrier.onClose(closed);
    channel.deliver("not json at all");
    expect(channel.readyState).toBe("closed");
    expect(closed).toHaveBeenCalledTimes(1);
  });

  it("lets a bug in the reassembler out rather than passing it off as a protocol violation", () => {
    const channel = new FakeChannel();
    const carrier = openCarrier({ channel });
    carrier.onEnvelope(() => {
      throw new RangeError("a listener that is broken");
    });
    expect(() => channel.deliver(JSON.stringify(envelope))).toThrow(RangeError);
  });
});

// A bridge reconnect drops the relay socket under whatever was in flight: the
// frame whose encryption was pending, the ping the liveness watcher had already
// issued. Each of those resumes and writes, and the browser logs the write on a
// dead wire. The refusal is the carrier's, in the words the channel carrier
// refuses in, so the call it belonged to fails at once rather than waiting out
// a reply nobody will send.
describe("a relay carrier whose socket has gone", () => {
  it("refuses the envelope rather than writing to a dead wire", () => {
    const socket = new FakeSocket();
    const carrier = openCarrier({ socket, sessionId: "sess-1" });

    socket.close();

    expect(() => carrier.send({ id: 1 })).toThrow("the relay socket closed");
    expect(socket.sent).toEqual([]);
  });

  // The close event is not the first moment a socket stops taking writes: a
  // socket someone has called close() on is CLOSING straight away and reports
  // its close a turn later.
  it("refuses while the socket is still closing, before its close has landed", () => {
    const socket = new FakeSocket();
    const carrier = openCarrier({ socket, sessionId: "sess-1" });

    socket.readyState = 2; // CLOSING: no close event yet

    expect(() => carrier.send({ id: 1 })).toThrow("the relay socket closed");
    expect(socket.sent).toEqual([]);
  });
});
