// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import greetingFixture from "../../fixtures/api/v1/session.hello.json";
import { greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { compactionSupportAddress } from "../src/core/compactionSupport.js";
import { reviewSupportAddress } from "../src/core/taskReviewSupport.js";
import { taskWaitingOnSupportAddress, readTaskWaitingOnEverSupported, resetTaskWaitingOnHistory } from "../src/core/taskWaitingOnSupport.js";

const deviceId = "greeting-evidence";
const facts = [
  ["compaction", compactionSupportAddress(deviceId), "harnessThresholds"],
  ["reviews", reviewSupportAddress(deviceId), "get"],
  ["prerequisites", taskWaitingOnSupportAddress(deviceId), "waitingOn"],
];
const greet = (capabilities, api_version = greetingFixture.result.api_version) => greetBridge(async method => method === "session.hello"
  ? { ...greetingFixture.result, api_version, capabilities } : {}, { deviceId, strict: true });
const capable = () => greet(["compaction.harnessThresholds", "tasks.review.get", "tasks.waitingOn"]);

beforeEach(async () => {
  resetChangeEvents();
  resetTaskWaitingOnHistory();
  await wipeCache();
});
afterEach(() => resetChangeEvents());

it.each(facts)("keeps cached %s evidence after an unknown-method greeting refusal", async (_name, address, flag) => {
  await capable();
  await vi.waitFor(async () => expect((await readCached(address))?.value?.[flag]).toBe(true));
  const before = await readCached(address);
  await expect(greetBridge(async () => { throw new Error("unknown method: session.hello"); }, { deviceId, strict: true })).resolves.toBe(false);
  expect(await readCached(address)).toEqual(before);
  expect(await readTaskWaitingOnEverSupported(deviceId)).toBe(true);
});

it.each(facts)("writes false for %s when an accepted legacy greeting lacks the capability", async (_name, address, flag) => {
  await capable();
  await vi.waitFor(async () => expect((await readCached(address))?.value?.[flag]).toBe(true));
  await greet([], "3.24.0");
  await vi.waitFor(async () => expect((await readCached(address))?.value?.[flag]).toBe(false));
  expect(await readTaskWaitingOnEverSupported(deviceId)).toBe(true);
});
