// @vitest-environment jsdom
// The line the reader comes back to.
//
// A conversation the human has been away from opens on the first thing they
// have not read, and the timeline says why it landed there: one ruled line,
// above that message and below everything they have already seen.

import { describe, expect, it } from "vitest";
import { UNREAD_LINE_SELECTOR, paintThreadEntries, timelineEntries } from "../src/core/thread.js";

const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });

const keysOf = (built) => built.entries.map((entry) => entry.key);

describe("the unread line", () => {
  it("is ruled above the first message the reader has not read", () => {
    const items = [message(2, "one"), message(4, "two"), message(7, "three")];
    const built = timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 4 });
    expect(keysOf(built)).toEqual(["2", "unread", "4", "7"]);
  });

  it("never places the divider above a user message or event", () => {
    const items = [
      { type: "message", data: { sequence: 4, role: "user", body: "my request" } },
      { type: "event", data: { sequence: 5, event: "test" } },
      message(7, "agent reply"),
    ];
    const built = timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 4 });
    const index = keysOf(built).indexOf("unread");
    expect(keysOf(built)[index + 1]).toBe("7");
    expect(index).toBeGreaterThan(0);
  });

  it("counts for nothing in the conversation's own count", () => {
    const items = [message(2, "one"), message(4, "two")];
    const built = timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 4 });
    expect(built.itemCount).toBe(2);
  });

  it("rules nothing when the reader is caught up", () => {
    const items = [message(2, "one"), message(4, "two")];
    const built = timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: null });
    expect(keysOf(built)).toEqual(["2", "4"]);
  });

  it("stands above the whole window when everything in it is unread", () => {
    const items = [message(4, "two"), message(7, "three")];
    const built = timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 2 });
    expect(keysOf(built)).toEqual(["unread", "4", "7"]);
  });

  it("rules nothing above a conversation with nothing in it", () => {
    const built = timelineEntries([], "Claude", "thread-1", [], { unreadFrom: 2 });
    expect(keysOf(built)).toEqual([]);
  });

  it("is drawn into the timeline where the scroll can find it", () => {
    const host = document.createElement("div");
    const items = [message(2, "one"), message(4, "two")];
    paintThreadEntries(host, timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 4 }));
    const line = host.querySelector(UNREAD_LINE_SELECTOR);
    expect(line).toBeTruthy();
    expect(line.nextElementSibling.getAttribute("data-key")).toBe("4");
  });

  it("is taken away again when the reader catches up", () => {
    const host = document.createElement("div");
    const items = [message(2, "one"), message(4, "two")];
    paintThreadEntries(host, timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: 4 }));
    paintThreadEntries(host, timelineEntries(items, "Claude", "thread-1", [], { unreadFrom: null }));
    expect(host.querySelector(UNREAD_LINE_SELECTOR)).toBeNull();
  });
});
