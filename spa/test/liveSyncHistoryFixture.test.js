import { expect, it } from "vitest";
import { historyRepairSeed, historyRepairPage, historyReplayFrame } from "./browser/liveSyncHistoryFixture.mjs";

it("bursts the first two own tips without changing per-thread cursors or mean input", () => {
  const frames = Array.from({ length: 36 }, (_, frame) => historyReplayFrame(frame));
  expect(frames.slice(0, 2)).toEqual([{ rowIndex: 0, sequence: 1101 }, { rowIndex: 0, sequence: 1102 }]);
  for (let rowIndex = 0; rowIndex < 6; rowIndex++) {
    expect(frames.filter((frame) => frame.rowIndex === rowIndex).map((frame) => frame.sequence))
      .toEqual([1101, 1102, 1103, 1104, 1105, 1106]);
  }
});

it("owes historical repair after nineteen creations and one under-cursor change", () => {
  const seed = historyRepairSeed();
  expect(seed.held).toHaveLength(330);
  expect(seed.held.at(-1).data.sequence).toBe(1080);
  const page = historyRepairPage(seed, [], { after_sequence: 1080, newest: true, limit: 100 });
  expect(page.items).toHaveLength(19);
  expect(page.thread_last_sequence).toBe(1100);
  expect(page.has_more).toBe(false);
});

it("caps a repair before forty-six sparse held copies and answers exact reads", () => {
  const seed = historyRepairSeed();
  const page = historyRepairPage(seed, [], { after_sequence: 295, limit: 50 });
  expect(page.items).toHaveLength(50);
  expect(page.has_more).toBe(true);
  const missing = seed.held.slice(-50).filter((item) => !page.items.some((copy) => copy.data.sequence === item.data.sequence));
  expect(missing).toHaveLength(46);
  const copy = historyRepairPage(seed, [], { after_sequence: 999, limit: 1 });
  expect(copy.items[0].data).toMatchObject({ sequence: 1000, updated_sequence: 1100, body: "historically updated retained message" });
  expect(copy.has_more).toBe(true);
});

it("includes continuing creations and caps newest reads from the tail", () => {
  const seed = historyRepairSeed();
  const arriving = [{ type: "message", data: { id: "new", sequence: 1101, body: "new" } }];
  const page = historyRepairPage(seed, arriving, { after_sequence: 1080, newest: true, limit: 2 });
  expect(page.items.map((item) => item.data.sequence)).toEqual([1099, 1101]);
  expect(page.thread_last_sequence).toBe(1101);
  expect(page.has_more).toBe(true);
});
