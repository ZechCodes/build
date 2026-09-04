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

class FakeSocket extends FakeEventTarget {
  constructor() {
    super();
    this.sent = [];
    this.closed = false;
  }
  send(text) {
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.closed = true;
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

  it("reports the wire gone once, whether the socket dropped or we closed it", () => {
    const socket = new FakeSocket();
    const carrier = openCarrier({ socket, sessionId: "sess-1" });
    const closed = vi.fn();
    carrier.onClose(closed);
    carrier.close();
    socket.emit("close");
    expect(socket.closed).toBe(true);
    expect(closed).toHaveBeenCalledTimes(1);
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
