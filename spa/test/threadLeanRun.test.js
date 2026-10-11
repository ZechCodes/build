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
  expect(runDigestToFetch(window.activityDigests, 2, window.deliveredSequence, 100, window.items)).toEqual(lean ? digest : null);
});
