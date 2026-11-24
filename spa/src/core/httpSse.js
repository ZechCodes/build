// No durable cursor: the owner refreshes current state after a broken stream.
// Consumer promises apply backpressure directly; no extra client event queue.
const MAX_EVENT_BYTES = 1024 * 1024;

// The bridge sends a keepalive every 15 s. Allow 5 s for delivery jitter.
const STREAM_IDLE_MS = 20000;

export function createStreamWatch(onStall, now) {
  const page = globalThis.document;
  const window = globalThis.window;
  let lastByteAt;
  let timer;
  const arm = (delay) => {
    clearTimeout(timer);
    timer = setTimeout(onStall, delay);
  };
  const onWake = () => {
    if (page?.visibilityState === "hidden") return;
    // Wall time keeps counting when the browser freezes callbacks and timers.
    const remaining = STREAM_IDLE_MS - (now() - lastByteAt);
    if (remaining <= 0) onStall();
    else arm(Math.min(remaining, STREAM_IDLE_MS));
  };
  const onPageShow = (event) => {
    if (event.persisted) onStall();
  };
  return {
    activity() {
      lastByteAt = now();
      arm(STREAM_IDLE_MS);
    },
    watchWake() {
      page?.addEventListener("visibilitychange", onWake);
      window?.addEventListener("online", onWake);
      window?.addEventListener("pageshow", onPageShow);
    },
    close() {
      clearTimeout(timer);
      page?.removeEventListener("visibilitychange", onWake);
      window?.removeEventListener("online", onWake);
      window?.removeEventListener("pageshow", onPageShow);
    },
  };
}

const utf8Bytes = (character) => {
  const point = character.codePointAt(0);
  if (point <= 0x7f) return 1;
  if (point <= 0x7ff) return 2;
  return point <= 0xffff ? 3 : 4;
};

function eventParser(consume) {
  let event = "message";
  let lines = [];
  return async (line) => {
    if (line === "") {
      if (lines.length) await consume({ event, data: JSON.parse(lines.join("\n")) });
      event = "message"; lines = [];
      return;
    }
    if (line.startsWith(":")) return;
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
    if (field === "id") throw new Error("Durable SSE cursors are unsupported");
    if (field === "event") event = value;
    if (field === "data") lines.push(value);
  };
}

function lineParser(consume) {
  let pending = "";
  let pendingCr = false;
  let wireBytes = 0;
  const count = (bytes) => {
    wireBytes += bytes;
    if (wireBytes > MAX_EVENT_BYTES) throw new Error("SSE event exceeds its byte limit");
  };
  const endLine = async (endingBytes) => {
    count(endingBytes);
    await consume(pending);
    if (pending === "") wireBytes = 0;
    pending = "";
  };
  return {
    async write(text) {
      for (const character of text) {
        if (pendingCr) {
          pendingCr = false;
          await endLine(character === "\n" ? 2 : 1);
          if (character === "\n") continue;
        }
        if (character === "\r") pendingCr = true;
        else if (character === "\n") await endLine(1);
        else { count(utf8Bytes(character)); pending += character; }
      }
    },
    async finish() { if (pendingCr) await endLine(1); },
  };
}

function eventReader(response) {
  if (!response.ok || response.headers.get("content-type")?.split(";")[0].trim() !== "text/event-stream" || !response.body) {
    throw new Error("Expected an SSE response");
  }
  return response.body.getReader();
}

export async function readTransientEvents(response, consume, onActivity = () => {}, signal) {
  const reader = eventReader(response);
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const take = lineParser(eventParser(consume));
  const cancel = () => reader.cancel().catch(() => {});
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    while (true) {
      signal?.throwIfAborted();
      const { value, done } = await reader.read();
      signal?.throwIfAborted();
      if (done) { await take.write(decoder.decode()); await take.finish(); return; }
      if (value.byteLength) onActivity();
      await take.write(decoder.decode(value, { stream: true }));
    }
  } finally {
    signal?.removeEventListener("abort", cancel);
    await cancel();
    reader.releaseLock();
  }
}
