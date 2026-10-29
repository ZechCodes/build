import { describe, expect, it, vi } from "vitest";
import { bindCarrier, DC_BUFFERED_HIGH, openCarrier } from "../src/core/carrier.js";
import { CHUNK_BYTES, createReassembler } from "../src/core/chunk.js";
import { createSessionRpc } from "../src/core/sessionRpc.js";
import { transportEpoch } from "./transportEpoch.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

class Channel {
  listeners = {};
  sent = [];
  readyState = "open";
  bufferedAmount = 0;
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  emit(type, event = {}) { for (const fn of this.listeners[type] || []) fn(event); }
  send(text) { this.sent.push(text); }
  close() { this.readyState = "closed"; this.emit("close"); }
  deliver(frame) { this.emit("message", { data: JSON.stringify(frame) }); }
  drain() { this.bufferedAmount = 0; this.emit("bufferedamountlow"); }
}

// A binding codec with a visible sequence, so a canceled queued call cannot
// hide a hole in the actual ordered wire behind the RPC correlation layer.
function codec() {
  let sendSequence = 0;
  let receiveSequence = 0;
  return {
    encode: vi.fn((payload, frame_type = "data") => ({ binding: "app", seq: ++sendSequence, frame_type, payload })),
    decode: vi.fn((frame) => {
      if (frame.binding !== "app" || frame.seq !== receiveSequence + 1) throw new Error("invalid binding or sequence");
      receiveSequence++;
      return { frame_type: frame.frame_type, payload: frame.payload };
    }),
  };
}

function setup(options = {}) {
  const channel = new Channel();
  const raw = openCarrier({ channel });
  const binding = codec();
  const carrier = bindCarrier(raw, binding);
  const transport = {
    encryptFrame: vi.fn(async ({ frameFields }) => ({ encrypted: frameFields })),
    decryptEnvelope: vi.fn(async ({ envelope }) => envelope.encrypted),
  };
  const rpc = createSessionRpc({ transport, sessionId: "s", deviceId: "d", sessionKeyB64: "secret", bridgeEpoch: transportEpoch(), timeoutMs: 0, ...options });
  rpc.rideOn(carrier);
  return { channel, raw, binding, carrier, rpc, transport };
}

function written(channel) {
  const reassembler = createReassembler();
  return channel.sent.flatMap((part) => {
    const frame = reassembler.accept(part);
    return frame === null ? [] : [JSON.parse(frame)];
  });
}

describe("verified clear carrier RPC", () => {
  it("carries requests, replies, receipts and pushes without the application cipher", async () => {
    const { channel, rpc, transport } = setup();
    const pushed = vi.fn();
    const receipt = vi.fn();
    rpc.onPush(pushed);
    const result = rpc.call("project.list", {}, { onReceipt: receipt });
    await tick();
    const [request] = written(channel);
    expect(request.payload).toEqual({ method: "project.list", id: "r1", params: {} });
    channel.deliver({ binding: "app", seq: 1, frame_type: "data", payload: { id: "r1", accepted: true } });
    channel.deliver({ binding: "app", seq: 2, frame_type: "data", payload: { id: "r1", ok: true, result: ["project"] } });
    channel.deliver({ binding: "app", seq: 3, frame_type: "data", payload: { type: "changes" } });
    await expect(result).resolves.toEqual(["project"]);
    expect(receipt).toHaveBeenCalledOnce();
    expect(pushed).toHaveBeenCalledWith({ type: "changes" });
    expect(transport.encryptFrame).not.toHaveBeenCalled();
    expect(transport.decryptEnvelope).not.toHaveBeenCalled();
    rpc.close();
  });

  it("keeps relay signaling on the explicit encrypted path", async () => {
    const { rpc, transport } = setup();
    const socket = new Channel();
    socket.readyState = 1;
    const relay = openCarrier({ socket, sessionId: "s" });
    rpc.readFrom(relay);
    const answer = rpc.call("rtc.offer", {}, { carrier: relay });
    await tick();
    expect(JSON.parse(socket.sent[0]).envelope.encrypted.payload.method).toBe("rtc.offer");
    socket.emit("message", { data: JSON.stringify({ type: "e2ee_envelope", session_id: "s", envelope: { encrypted: { payload: { id: "r1", ok: true, result: "answer" } } } }) });
    await expect(answer).resolves.toBe("answer");
    expect(transport.encryptFrame).toHaveBeenCalledOnce();
    expect(transport.decryptEnvelope).toHaveBeenCalledOnce();
    rpc.close();
  });

  it("cancels a queued call before allocating its sequence or marking handoff", async () => {
    const { channel, rpc, binding } = setup();
    channel.bufferedAmount = DC_BUFFERED_HIGH + 1;
    const first = rpc.call("project.list");
    await tick();
    const abort = new AbortController();
    const canceled = rpc.call("term.input", {}, { signal: abort.signal });
    const failed = canceled.catch((error) => error);
    await tick();
    abort.abort();
    expect((await failed).uncertain).not.toBe(true);
    channel.drain();
    await tick();
    const third = rpc.call("project.list");
    await tick();
    expect(binding.encode).toHaveBeenCalledTimes(2);
    expect(written(channel).map((frame) => [frame.seq, frame.payload.id])).toEqual([[1, "r1"], [2, "r3"]]);
    channel.deliver({ binding: "app", seq: 1, frame_type: "data", payload: { id: "r1", ok: true } });
    channel.deliver({ binding: "app", seq: 2, frame_type: "data", payload: { id: "r3", ok: true } });
    await Promise.all([first, third]);
    rpc.close();
  });

  it("keeps SQL credit until the final reply after a receipt", async () => {
    const { channel, rpc, binding } = setup({ sqlReadLimit: 1 });
    const first = rpc.call("tasks.list");
    const next = rpc.call("tasks.get");
    await tick();
    expect(binding.encode).toHaveBeenCalledOnce();
    channel.deliver({ binding: "app", seq: 1, frame_type: "data", payload: { id: "r1", accepted: true } });
    await tick();
    expect(rpc.sqlReadCreditSnapshot()).toMatchObject({ admitted: 1, queued: 1 });
    expect(binding.encode).toHaveBeenCalledOnce();
    channel.deliver({ binding: "app", seq: 2, frame_type: "data", payload: { id: "r1", ok: true } });
    await first;
    await tick();
    expect(binding.encode).toHaveBeenCalledTimes(2);
    channel.deliver({ binding: "app", seq: 3, frame_type: "data", payload: { id: "r2", ok: true } });
    await next;
    expect(rpc.sqlReadCreditSnapshot()).toMatchObject({ admitted: 0, queued: 0 });
    rpc.close();
  });

  it("marks a partial clear application write as uncertain", async () => {
    const { channel, rpc } = setup();
    channel.send = (text) => { if (channel.sent.length) throw new Error("partial write"); channel.sent.push(text); };
    const error = await rpc.call("term.input", { text: "x".repeat(CHUNK_BYTES * 2) }).catch((failure) => failure);
    expect(error).toMatchObject({ message: "partial write", uncertain: true });
    rpc.close();
  });

  it("rechecks caller generation at sequence allocation", async () => {
    const { carrier, binding, channel, rpc } = setup();
    channel.bufferedAmount = DC_BUFFERED_HIGH + 1;
    const first = carrier.sendFrame({ first: true });
    await tick();
    const stale = carrier.sendFrame({}, { assertCurrent: () => { throw new Error("old generation"); } });
    const failed = stale.catch((error) => error);
    channel.drain();
    await first;
    expect((await failed).message).toBe("old generation");
    await carrier.sendFrame({ next: true });
    expect(binding.encode).toHaveBeenCalledTimes(2);
    expect(channel.readyState).toBe("open");
    rpc.close();
  });
});

describe("bound carrier retirement", () => {
  it("closes the exact carrier after partial chunk failure and cannot resume", async () => {
    const { channel, carrier, binding, rpc } = setup();
    const close = vi.fn();
    carrier.onClose(close);
    channel.send = (text) => { if (channel.sent.length) throw new Error("partial write"); channel.sent.push(text); };
    await expect(carrier.sendFrame({ text: "x".repeat(CHUNK_BYTES * 2) })).rejects.toThrow("partial write");
    expect(channel.readyState).toBe("closed");
    expect(close).toHaveBeenCalledOnce();
    await expect(carrier.sendFrame({ later: true })).rejects.toThrow();
    expect(binding.encode).toHaveBeenCalledOnce();
    rpc.close();
  });

  it("retires a failed encoder without attempting a handoff", async () => {
    const { carrier, binding, channel, rpc } = setup();
    binding.encode.mockImplementation(() => { throw new Error("sequence exhausted"); });
    const handoff = vi.fn();
    await expect(carrier.sendFrame({}, { onHandoff: handoff })).rejects.toThrow("sequence exhausted");
    expect(handoff).not.toHaveBeenCalled();
    expect(channel.readyState).toBe("closed");
    rpc.close();
  });

  it("invalidates decode failures and suppresses all later callbacks", () => {
    const { channel, carrier, binding, rpc } = setup();
    const frame = vi.fn();
    const close = vi.fn();
    carrier.onFrame(frame);
    carrier.onClose(close);
    channel.deliver({ binding: "wrong", seq: 1, frame_type: "data", payload: {} });
    channel.deliver({ binding: "app", seq: 1, frame_type: "data", payload: { type: "changes" } });
    expect(frame).not.toHaveBeenCalled();
    expect(binding.decode).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledOnce();
    expect(channel.readyState).toBe("closed");
    rpc.close();
  });

  it("forwards liveness and retires on raw closure while a send is queued", async () => {
    const { channel, carrier, binding, rpc } = setup();
    expect(carrier.peerFrameAt()).toBe(0);
    expect(carrier.peerIsConnected()).toBe(false);
    const pending = carrier.sendFrame({});
    channel.close();
    await expect(pending).rejects.toThrow();
    expect(binding.encode).not.toHaveBeenCalled();
    rpc.close();
  });
});
