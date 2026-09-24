// How much of a held conversation the panel draws (#158).
//
// The record holds everything the reader ever widened into, thousands of items
// on a busy agent, and the panel used to build a row for every one of them on
// open. It draws the newest entries now, a folded run counting once, and more
// as the reader asks.

import { describe, expect, it } from "vitest";
import {
  EARLIER_ENTRY_KEY,
  TIMELINE_SLICE_SIZE,
  createTimelineSlice,
  sliceTimeline,
} from "../src/core/timelineSlice.js";
import { timelineEntries } from "../src/core/thread.js";

const said = (sequence, role = "agent") => ({
  type: "message",
  data: { sequence, id: `m-${sequence}`, role, body: `said ${sequence}` },
});
const call = (sequence) => ({ type: "event", data: { sequence, event: "tool_use", summary: `Read ${sequence}.js` } });

const messages = (count, from = 1) => Array.from({ length: count }, (_, index) => said(from + index));

/** A conversation of `turns` messages, each followed by a run of `calls` tool
 *  calls: two entries a turn, however many calls the run holds. */
const turnsWithRuns = (turns, calls) =>
  Array.from({ length: turns }, (_, turn) => {
    const base = turn * (calls + 1) + 1;
    return [said(base), ...Array.from({ length: calls }, (_, index) => call(base + 1 + index))];
  }).flat();

const keysOf = (built) => built.entries.map((entry) => entry.key);
/** The conversation's own entries: not the row above them, nor the line. */
const drawnKeys = (built) => keysOf(built).filter((key) => key !== EARLIER_ENTRY_KEY && key !== "unread");

/** Build the way the panel does: slice with what it remembers, then settle. */
const paint = (items, slice, options = {}) => {
  const built = timelineEntries(items, "Claude Code", "th-1", [], { slice: slice.request(), ...options });
  slice.settle(built.sliced);
  return built;
};

describe("the slice a conversation is drawn from", () => {
  it("keeps the newest entries and a row above them that shows more", () => {
    const built = paint(messages(200), createTimelineSlice());

    expect(drawnKeys(built)).toHaveLength(TIMELINE_SLICE_SIZE);
    expect(drawnKeys(built)[0]).toBe(String(200 - TIMELINE_SLICE_SIZE + 1));
    expect(drawnKeys(built).at(-1)).toBe("200");
    expect(keysOf(built)[0]).toBe(EARLIER_ENTRY_KEY);
    expect(built.sliced.hidden).toBe(200 - TIMELINE_SLICE_SIZE);
    // The title still counts the whole conversation, not what is drawn of it.
    expect(built.itemCount).toBe(200);
  });

  it("counts a folded run of activity as one entry, however many calls it holds", () => {
    const built = paint(turnsWithRuns(100, 5), createTimelineSlice());

    expect(drawnKeys(built)).toHaveLength(TIMELINE_SLICE_SIZE);
    const runs = built.entries.filter((entry) => entry.html.includes("thread-activity-group"));
    expect(runs).toHaveLength(TIMELINE_SLICE_SIZE / 2);
  });

  it("shows the next entries above the floor each time the reader asks", () => {
    const slice = createTimelineSlice();
    const items = messages(200);
    paint(items, slice);

    slice.showEarlier();
    const built = paint(items, slice);

    expect(drawnKeys(built)).toHaveLength(2 * TIMELINE_SLICE_SIZE);
    expect(drawnKeys(built)[0]).toBe(String(200 - 2 * TIMELINE_SLICE_SIZE + 1));
    expect(slice.hasHiddenEntries()).toBe(true);
  });

  it("hands the ask to the bridge once the cache is drawn to its start", () => {
    const slice = createTimelineSlice();
    const items = messages(90);
    paint(items, slice);
    slice.showEarlier();

    const drawnWhole = paint(items, slice);
    expect(drawnKeys(drawnWhole)).toHaveLength(90);
    // Nothing left above in the cache: the next ask is the load-older fetch.
    expect(slice.hasHiddenEntries()).toBe(false);
    expect(keysOf(drawnWhole)).not.toContain(EARLIER_ENTRY_KEY);

    // Unless the bridge holds more, and then the row stays to ask for it.
    const withMoreOnBridge = paint(items, slice, { olderOnBridge: true });
    expect(keysOf(withMoreOnBridge)[0]).toBe(EARLIER_ENTRY_KEY);
  });

  it("grows at the bottom when a push lands, and never drops its top", () => {
    const slice = createTimelineSlice();
    const first = paint(messages(200), slice);
    const top = drawnKeys(first)[0];

    const afterPush = paint(messages(205), slice);

    expect(drawnKeys(afterPush)[0]).toBe(top);
    expect(drawnKeys(afterPush)).toHaveLength(TIMELINE_SLICE_SIZE + 5);
  });

  it("keeps a run that older history extended upwards, as one row", () => {
    // The floor was the run's start, 5, before a page above it joined 2-4 on.
    const entries = [
      { key: "1", from: 1, through: 1 },
      { key: "2", from: 2, through: 12 },
      { key: "13", from: 13, through: 13 },
    ];

    const sliced = sliceTimeline(entries, { floor: 5 });

    expect(sliced.entries.map((entry) => entry.key)).toEqual(["2", "13"]);
    expect(sliced.floor).toBe(2);
  });

  it("reaches down to the unread line, wherever the floor stood", () => {
    const slice = createTimelineSlice();
    const built = paint(messages(200), slice, { unreadFrom: 50 });

    expect(drawnKeys(built)[0]).toBe("50");
    expect(keysOf(built)).toContain("unread");
    // And that is the floor now: the next paint does not take it back.
    expect(drawnKeys(paint(messages(200), slice))[0]).toBe("50");
  });

  it("reaches down to a sequence asked for, and holds there", () => {
    const slice = createTimelineSlice();
    const items = messages(200);
    paint(items, slice);

    slice.reachDown(7);
    expect(drawnKeys(paint(items, slice))[0]).toBe("7");
    expect(drawnKeys(paint(items, slice))[0]).toBe("7");
  });

  it("draws everything for a caller that asks for no slice", () => {
    const built = timelineEntries(messages(200), "Claude Code", "th-1", []);

    expect(keysOf(built)).toHaveLength(200);
    expect(keysOf(built)).not.toContain(EARLIER_ENTRY_KEY);
  });

  it("maps every one of the reader's messages in the tick column, drawn or not", () => {
    const items = Array.from({ length: 200 }, (_, index) => said(index + 1, index % 2 ? "agent" : "user"));
    const built = paint(items, createTimelineSlice());

    expect(built.userTicks).toHaveLength(100);
  });
});

describe("the markup a slice builds", () => {
  /** Items whose markup reports being written: the reader's message body and a
   *  call's links are read by the row's html and by nothing else. */
  const counted = () => {
    const reads = { count: 0 };
    const counting = (item, field) => {
      Object.defineProperty(item.data, field, {
        enumerable: true,
        get: () => {
          reads.count += 1;
          return field === "links" ? [] : `said ${item.data.sequence}`;
        },
      });
      return item;
    };
    return { reads, counting };
  };

  it("writes a row's markup only for the entries it draws", () => {
    const { reads, counting } = counted();
    const items = Array.from({ length: 500 }, (_, index) => {
      const item = { type: "message", data: { sequence: index + 1, id: `m-${index + 1}`, role: "agent" } };
      return counting(item, "body");
    });

    const built = paint(items, createTimelineSlice());
    built.entries.forEach((entry) => entry.html);

    expect(reads.count).toBe(TIMELINE_SLICE_SIZE);
  });

  it("draws a shut run as its head alone, and builds its rows when it opens", () => {
    const { reads, counting } = counted();
    const items = [said(1), ...Array.from({ length: 40 }, (_, index) => counting(call(index + 2), "links"))];

    const shut = timelineEntries(items, "Claude Code", "th-1", [], { slice: createTimelineSlice().request() });
    const [, run] = shut.entries;
    expect(run.html).toContain("thread-activity-group-head");
    expect(run.html).not.toContain("thread-activity-group-list");
    expect(reads.count).toBe(0);

    const open = timelineEntries(items, "Claude Code", "th-1", [], {
      slice: createTimelineSlice().request(),
      openRuns: new Set(["2"]),
    });
    expect(open.entries[1].html).toContain("thread-activity-group-list");
    expect(reads.count).toBe(40);
  });
});
