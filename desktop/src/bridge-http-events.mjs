import { randomUUID } from "node:crypto";
import {
  networkOptions,
  consumeEvents,
  MAX_JSON_BYTES,
} from "./bridge-http-network.mjs";
import { fail, exact, identifier, segment } from "./bridge-http-contract.mjs";

export function createBridgeEventStreams({
  fetch,
  check,
  headers,
  send,
  device,
  handleFor,
  retire,
}) {
  const streams = new Map();
  function endStream(stream, reason) {
    if (!streams.delete(stream.id)) return;
    stream.handle.streams.delete(stream.id);
    if (stream.handle.stream === stream.id) stream.handle.stream = null;
    clearTimeout(stream.stall);
    stream.controller.abort();
    stream.unacked.clear();
    stream.bytes = 0;
    if (reason) {
      try {
        send(stream.handle.owner, {
          stream: stream.id,
          sequence: ++stream.sequence,
          event: "reset_required",
          data: { reason, recovery: "current_state_refresh" },
        });
      } catch {}
    }
  }
  function validateEvent(stream, event) {
    if (event.event === "hello") {
      if (
        stream.ready ||
        event.data.durable !== false ||
        event.data.recovery !== "current_state_refresh" ||
        event.data.subscription_id !== stream.subscription ||
        event.data.opening_generation !==
          stream.handle.opening.opening_generation
      )
        fail("invalid_stream");
      stream.ready = true;
    } else if (!stream.ready) fail("invalid_stream");
  }
  function emit(stream, event) {
    check(stream.handle);
    if (!streams.has(stream.id)) fail("canceled");
    validateEvent(stream, event);
    const size = Buffer.byteLength(JSON.stringify(event));
    if (stream.unacked.size >= 128 || stream.bytes + size > MAX_JSON_BYTES) {
      endStream(stream, "overflow");
      fail("stream_too_large");
    }
    stream.bytes += size;
    stream.unacked.set(++stream.sequence, size);
    if (!stream.stall) {
      stream.stall = setTimeout(() => endStream(stream, "stalled"), 10000);
      stream.stall.unref?.();
    }
    send(stream.handle.owner, {
      stream: stream.id,
      sequence: stream.sequence,
      ...event,
    });
  }
  async function pump(stream) {
    try {
      if (!streams.has(stream.id)) return;
      check(stream.handle);
      const timer = setTimeout(() => stream.controller.abort(), 5000);
      let response;
      try {
        response = await fetch(
          `${stream.handle.mint.origin}/v1/subscriptions/${segment(stream.subscription)}/events`,
          networkOptions({
            signal: stream.controller.signal,
            headers: { ...headers(stream.handle), Accept: "text/event-stream" },
          }),
        );
      } finally {
        clearTimeout(timer);
      }
      if (response.status === 401) {
        retire(stream.handle);
        void response.body?.cancel();
        return;
      }
      await consumeEvents(response, (event) => emit(stream, event), {
        signal: stream.controller.signal,
      });
      endStream(stream, "disconnected");
    } catch {
      endStream(stream, "disconnected");
    }
  }
  async function subscribe(owner, id, scope) {
    const handle = handleFor(owner, id);
    if (!handle.opening) fail("not_open");
    exact(scope, ["kind"]);
    if (scope.kind !== "application") fail("invalid_params");
    if (handle.streams.size || handle.subscribing) fail("already_subscribed");
    handle.subscribing = true;
    let result;
    try {
      result = await device(handle, "/v1/subscriptions", {
        method: "POST",
        body: scope,
        maxBody: 64 * 1024,
        headers: handle.lastSubscription
          ? { "X-Build-Subscription": handle.lastSubscription }
          : {},
      });
    } catch (error) {
      retire(handle);
      throw error;
    } finally {
      handle.subscribing = false;
    }
    identifier(result.subscription_id, "invalid_response");
    handle.lastSubscription = result.subscription_id;
    if (result.opening_generation !== handle.opening.opening_generation)
      fail("binding_mismatch");
    const path = `/v1/subscriptions/${segment(result.subscription_id)}/events`;
    if (result.events_path !== undefined && result.events_path !== path)
      fail("binding_mismatch");
    const stream = {
      id: randomUUID(),
      handle,
      subscription: result.subscription_id,
      controller: new AbortController(),
      sequence: 0,
      unacked: new Map(),
      bytes: 0,
    };
    streams.set(stream.id, stream);
    handle.streams.add(stream.id);
    handle.stream = stream.id;
    // Begin on the next turn so invoke returns the local stream id first.
    setImmediate(() => void pump(stream));
    return { stream: stream.id };
  }
  function ownedStream(owner, id) {
    const stream = streams.get(id);
    if (!stream || stream.handle.owner !== owner) fail("invalid_stream");
    handleFor(owner, stream.handle.id);
    return stream;
  }
  function ack(owner, id, sequence) {
    const stream = ownedStream(owner, id);
    if (!Number.isSafeInteger(sequence) || !stream.unacked.has(sequence))
      fail("invalid_ack");
    for (const [key, size] of stream.unacked) {
      if (key > sequence) break;
      stream.bytes -= size;
      stream.unacked.delete(key);
    }
    clearTimeout(stream.stall);
    stream.stall = null;
    if (stream.unacked.size) {
      stream.stall = setTimeout(() => endStream(stream, "stalled"), 10000);
      stream.stall.unref?.();
    }
  }
  async function unsubscribe(owner, id) {
    const stream = ownedStream(owner, id);
    endStream(stream);
    await device(
      stream.handle,
      `/v1/subscriptions/${segment(stream.subscription)}`,
      { method: "DELETE" },
    );
  }

  function clear(handle, reason) {
    for (const id of handle.streams) endStream(streams.get(id), reason);
  }
  function requestHeader(handle, requiresEvents) {
    const stream = streams.get(handle.stream);
    if (requiresEvents && !stream?.ready) fail("stream_required");
    return stream?.ready ? { "X-Build-Subscription": stream.subscription } : {};
  }
  return { subscribe, ack, unsubscribe, clear, requestHeader };
}
