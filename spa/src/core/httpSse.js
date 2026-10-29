// No durable cursor: the owner refreshes current state after a broken stream.
// Consumer promises apply backpressure directly; no extra client event queue.
const MAX_EVENT_BYTES = 1024 * 1024;
const encodedBytes = (value) => new TextEncoder().encode(value).length;

function eventParser(consume) {
  let event = "message";
  let lines = [];
  let size = 0;
  return async (line) => {
    size += encodedBytes(line) + 1;
    if (size > MAX_EVENT_BYTES) throw new Error("SSE event exceeds its byte limit");
    if (line === "") {
      if (lines.length) await consume({ event, data: JSON.parse(lines.join("\n")) });
      event = "message"; lines = []; size = 0;
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
  let skipLf = false;
  return async (text) => {
    for (const character of text) {
      if (skipLf && character === "\n") { skipLf = false; continue; }
      skipLf = character === "\r";
      if (character === "\r" || character === "\n") {
        await consume(pending); pending = "";
      } else {
        pending += character;
        if (pending.length > MAX_EVENT_BYTES) throw new Error("SSE line exceeds its byte limit");
      }
    }
  };
}

export async function readTransientEvents(response, consume) {
  if (!response.ok || response.headers.get("content-type")?.split(";")[0].trim() !== "text/event-stream" || !response.body) {
    throw new Error("Expected an SSE response");
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const take = lineParser(eventParser(consume));
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) { await take(decoder.decode()); return; }
      await take(decoder.decode(value, { stream: true }));
    }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
