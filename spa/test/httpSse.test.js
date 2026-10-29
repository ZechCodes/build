import { describe, expect, it } from "vitest";
import { readTransientEvents } from "../src/core/httpSse.js";

const bytes = new TextEncoder();
const response = (...parts) => new Response(new ReadableStream({
  start(controller) { for (const part of parts) controller.enqueue(bytes.encode(part)); controller.close(); },
}), { headers: { "content-type": "text/event-stream" } });

describe("bounded transient SSE reader", () => {
  it("handles CRLF boundaries, comments and multiline data serially", async () => {
    const events = [];
    await readTransientEvents(response(": heartbeat\r", "\nevent: changes\r\ndata: {\"type\":\n", "data: \"changes\"}\r\n\r", "\n"), async (event) => {
      await Promise.resolve(); events.push(event);
    });
    expect(events).toEqual([{ event: "changes", data: { type: "changes" } }]);
  });

  it("keeps UTF-8 codepoints across chunks", async () => {
    const data = bytes.encode('event: changes\ndata: {"label":"é"}\n\n');
    const split = data.indexOf(0xc3) + 1;
    const body = new ReadableStream({ start(c) { c.enqueue(data.slice(0, split)); c.enqueue(data.slice(split)); c.close(); } });
    const events = [];
    await readTransientEvents(new Response(body, { headers: { "content-type": "text/event-stream" } }), (event) => events.push(event));
    expect(events[0].data.label).toBe("é");
  });

  it.each(["id: 7\n\n", "data: invalid\n\n", `data: ${"x".repeat(1024 * 1024 + 1)}\n\n`])("rejects unsupported cursors, malformed and oversized events", async (input) => {
    await expect(readTransientEvents(response(input), () => {})).rejects.toThrow();
  });

  it("cancels the reader when the consumer refuses a frame", async () => {
    let cancelled = false;
    const body = new ReadableStream({ start(c) { c.enqueue(bytes.encode("data: {}\n\n")); }, cancel() { cancelled = true; } });
    await expect(readTransientEvents(new Response(body, { headers: { "content-type": "text/event-stream" } }), () => { throw Error("scope ended"); })).rejects.toThrow("scope ended");
    expect(cancelled).toBe(true);
  });

  it("rejects non-event responses", async () => {
    await expect(readTransientEvents(new Response("{}"), () => {})).rejects.toThrow();
  });

  it("counts exact CRLF wire bytes including framing at the event limit", async () => {
    const prefix = 'event: changes\r\ndata: {"text":"';
    const suffix = '"}\r\n\r\n';
    const padding = 1024 * 1024 - prefix.length - suffix.length;
    const events = [];
    await readTransientEvents(response(prefix + "x".repeat(padding) + suffix), (event) => events.push(event));
    expect(events).toHaveLength(1);
    await expect(readTransientEvents(response(prefix + "x".repeat(padding + 1) + suffix), () => {})).rejects.toThrow();
  });
});
