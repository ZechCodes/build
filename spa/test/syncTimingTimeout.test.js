// #869 investigation: distinguish caller timeout from bridge-work completion.
import { afterEach, expect, it, vi } from "vitest";
import { createSessionRpc, ANSWER_TIMEOUT_MS } from "../src/core/sessionRpc.js";
import { SQL_READ_LIMIT } from "../src/core/sqlReadCredits.js";
import { transportEpoch } from "./transportEpoch.js";

let rpc;
afterEach(() => { rpc?.close(new Error("test ended")); vi.useRealTimers(); });
const flush = async () => { for (let turn = 0; turn < 30; turn += 1) await Promise.resolve(); };

function setup() {
  vi.useFakeTimers();
  const listeners = new Set(), sent = [];
  const wire = { send: (frame) => sent.push(frame.payload),
    onEnvelope: (fn) => { listeners.add(fn); return () => listeners.delete(fn); }, onClose: () => () => {} };
  rpc = createSessionRpc({ bridgeEpoch: transportEpoch(), deviceId: "synthetic", sessionId: "synthetic",
    sessionKeyB64: "synthetic", transport: {
      encryptFrame: async ({ frameFields }) => frameFields,
      decryptEnvelope: async ({ envelope }) => envelope,
    } });
  rpc.rideOn(wire);
  const deliver = (payload) => { for (const listener of listeners) listener({ payload }); };
  const ask = (method) => {
    let outcome = "pending";
    const settled = rpc.call(method).then(() => { outcome = "answered"; }, (error) => { outcome = error; });
    return { settled, outcome: () => outcome };
  };
  return { sent, deliver, ask };
}

it.each(["thread.page", "tasks.list", "git.status", "fs.tree"])(
  "an unanswered receipted %s settles at 120s, not the 12s path deadline", async (method) => {
    const { sent, deliver, ask } = setup();
    const call = ask(method); await flush();
    deliver({ id: sent[0].id, accepted: true }); await flush();
    await vi.advanceTimersByTimeAsync(ANSWER_TIMEOUT_MS - 1);
    expect(call.outcome()).toBe("pending");
    await vi.advanceTimersByTimeAsync(1); await call.settled;
    expect(call.outcome()).toMatchObject({ timedOut: true, deadline: "answer" });
    console.log(JSON.stringify({ method, receiptedUnansweredMs: ANSWER_TIMEOUT_MS, outcome: "answer timeout" }));
  },
);

it("eight timed-out sent SQL reads leave a ninth unsent indefinitely until late reply/reset", async () => {
  const { sent, deliver, ask } = setup();
  const admitted = Array.from({ length: SQL_READ_LIMIT }, () => ask("thread.page"));
  const ninth = ask("tasks.list"); await flush();
  expect(sent).toHaveLength(8);
  for (const call of sent) deliver({ id: call.id, accepted: true }); await flush();
  await vi.advanceTimersByTimeAsync(ANSWER_TIMEOUT_MS);
  await Promise.all(admitted.map((call) => call.settled));
  expect(admitted.every((call) => call.outcome().deadline === "answer")).toBe(true);
  await vi.advanceTimersByTimeAsync(ANSWER_TIMEOUT_MS * 3);
  expect(sent).toHaveLength(8); expect(ninth.outcome()).toBe("pending");
  // Controls and writes bypass SQL admission even while SQL readers are stuck.
  const write = ask("thread.post"); await flush();
  expect(sent.at(-1).method).toBe("thread.post");
  deliver({ id: sent.at(-1).id, ok: true, result: {} }); await write.settled;
  deliver({ id: sent[0].id, ok: true, result: {} }); await flush();
  expect(sent.at(-1).method).toBe("tasks.list");
  rpc.resetSqlReads(); await ninth.settled;
  expect(ninth.outcome()).toMatchObject({ uncertain: true, retryable: true });
  console.log(JSON.stringify({ sentSqlTimedOut: 8, queuedSqlStillUnsentAfterMs: ANSWER_TIMEOUT_MS * 4,
    nonSqlWriteDispatched: true, releases: ["late reply", "authoritative reset"] }));
});
