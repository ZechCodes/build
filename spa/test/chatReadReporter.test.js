import { expect, it, vi } from "vitest";
import { createChatReadReporter } from "../src/core/chatReadReporter.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

it("retains newer confirmations when an older report finishes later", async () => {
  const reporter = createChatReadReporter();
  const older = deferred();
  const newer = deferred();
  const send = vi.fn().mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
  const first = reporter.report(11, 1, send);
  const second = reporter.report(12, 1, send);
  newer.resolve(true);
  await second;
  older.resolve(true);
  await first;
  await reporter.report(12, 1, send);
  expect(send).toHaveBeenCalledTimes(2);
});

it("retries a rejected report and reports a wider history window", async () => {
  let now = 0;
  const reporter = createChatReadReporter(() => now);
  const send = vi.fn().mockRejectedValueOnce(new Error("lost reply")).mockResolvedValue(true);
  await reporter.report(12, 11, send);
  now = 1000;
  await reporter.report(12, 11, send);
  await reporter.report(12, 1, send);
  await reporter.report(12, 11, send);
  expect(send).toHaveBeenCalledTimes(3);
});

it("does not let a pending report hide a newer viewport with a different floor", async () => {
  const reporter = createChatReadReporter();
  const pending = deferred();
  const send = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(true);
  const first = reporter.report(12, 11, send);
  await reporter.report(12, 1, send);
  expect(send).toHaveBeenCalledTimes(2);
  pending.resolve(false);
  await first;
  await reporter.report(12, 1, send);
  expect(send).toHaveBeenCalledTimes(2);
});

it("does not synthesize a confirmed viewport from a tail report and an earlier history report", async () => {
  const reporter = createChatReadReporter();
  const send = vi.fn().mockResolvedValue(true);
  await reporter.report(12, 11, send);
  await reporter.report(10, 1, send);
  await reporter.report(12, 1, send);
  expect(send).toHaveBeenCalledTimes(3);
});

it("caps refusal backoff at thirty seconds without scheduling a retry loop", async () => {
  let now = 0;
  const reporter = createChatReadReporter(() => now);
  const send = vi.fn().mockResolvedValue(false);
  for (const delay of [1000, 2000, 4000, 8000, 16000, 30000, 30000]) {
    const before = send.mock.calls.length;
    await reporter.report(12, 1, send);
    expect(send).toHaveBeenCalledTimes(before + 1);
    now += delay - 1;
    await reporter.report(12, 1, send);
    expect(send).toHaveBeenCalledTimes(before + 1);
    now += 1;
  }
});

it("retries a tail report once reading earlier attention makes its cursor advance safe", async () => {
  const reporter = createChatReadReporter();
  let cursor = 0;
  const attention = Array.from({ length: 120 }, (_, index) => index + 1);
  const history = [];
  // bridge/src/thread/conversation.rs unread_attention_below +
  // bridge/src/app/board/attention.rs conversation_last_sequences/entity_seen:
  // a hidden unread attention item prevents cursor advancement, but the RPC
  // still answers {ok:true}; inboxSeen maps any fulfilled RPC to true.
  const markSeen = vi.fn(async (read, floor) => {
    const hiddenBelow = attention.some((sequence) => sequence > cursor && sequence < floor);
    if (!hiddenBelow) cursor = Math.max(cursor, Math.min(read, 120));
    history.push({ read, floor, hiddenBelow, cursor });
    return true;
  });
  await reporter.report(120, 61, () => markSeen(120, 61), () => cursor);
  expect(cursor).toBe(0);
  await reporter.report(60, 1, () => markSeen(60, 1), () => cursor);
  expect(cursor).toBe(60);
  await reporter.report(120, 61, () => markSeen(120, 61), () => cursor);
  expect.soft(markSeen).toHaveBeenCalledTimes(3);
  expect.soft(cursor).toBe(120);
});
