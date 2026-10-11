import { expect, it } from "vitest";
import { threadWindow } from "../src/core/threadSync.js";
import { runDigestToFetch } from "../src/core/activityDigest.js";

const activity = (sequence) => ({ type: "event", data: { sequence, event: "tool_use", summary: `Read ${sequence}.js` } });
const message = (sequence) => ({ type: "message", data: { sequence, role: "agent", body: "Done" } });
const digest = { from_sequence: 2, through_sequence: 100, rows: 99, tool_calls: 99 };
const held = {
  items: [message(1), activity(2)], deliveredSequence: 2,
  activityDigests: [{ from_sequence: 2, through_sequence: 2, rows: 1, tool_calls: 1 }],
};

it.each([true, false])("opens a warm historical run after a lean=%s arrival", (lean) => {
  const arrivingActivity = lean ? [activity(100)] : Array.from({ length: 98 }, (_, index) => activity(index + 3));
  const window = threadWindow(held, {
    items: [...arrivingActivity, message(101)], thread_last_sequence: 101,
    has_more: lean, activity_digests: [digest],
  }, { newest: true, lean });
  expect(window.items.map((item) => item.data.sequence)).toEqual(lean ? [1, 2, 100, 101] : Array.from({ length: 101 }, (_, index) => index + 1));
  expect(window.activityDigests).toEqual([digest]);
  expect(runDigestToFetch(window.activityDigests, 2, window.items, 100)).toEqual(lean ? digest : null);
});

it.each([2, 100])("keeps a sparse live tail window-backed after row %s updates", (updatedRow) => {
  const items = [message(1), activity(2), activity(100)].map((item) => item.data.sequence === updatedRow
    ? { ...item, data: { ...item.data, updated_sequence: 101, outcome: "ok" } }
    : item);
  const window = threadWindow({ ...held, items, activityDigests: [digest] }, {
    items: [items.find((item) => item.data.sequence === updatedRow)], thread_last_sequence: 101, has_more: false,
  });
  expect(window.deliveredSequence).toBe(101);
  expect(runDigestToFetch(window.activityDigests, 2, window.items, 100)).toBe(null);
});
