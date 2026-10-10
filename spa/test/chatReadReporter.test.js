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
