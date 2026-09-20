import { describe, it, expect } from "vitest";
import {
  DETAIL_LEVELS,
  DETAIL_LEVEL_KEY_PREFIX,
  defaultDetailLevel,
  detailLevelKey,
  detailLevelMenuOptions,
  detailLevelOfOptionId,
  itemIsShownAt,
  itemsAtDetailLevel,
  readDetailLevel,
  readThroughHiddenItems,
  writeDetailLevel,
} from "../src/core/conversationDetail.js";

const message = (sequence, data = {}) => ({ type: "message", data: { sequence, role: "agent", ...data } });
const userSaid = (sequence) => message(sequence, { role: "user", body: "hi" });
const agentSaid = (sequence) => message(sequence, { role: "agent", body: "on it" });
const arrived = (sequence) => message(sequence, { role: "user", from_agent: { id: "other", topic: "Deploy" } });
const sentOut = (sequence) => message(sequence, { role: "agent", sent_to: { id: "other", topic: "Deploy" } });
const fromBuild = (sequence) => message(sequence, { role: "user", body: "The Build bridge restarted.", from_build: true });
const toolCall = (sequence) => ({ type: "tool_use", data: { sequence, event: "tool_use", summary: "read file" } });
const committed = (sequence) => ({ type: "committed", data: { sequence, event: "committed", summary: "abc123" } });

const storageThatRefuses = () => ({
  getItem() {
    throw new Error("storage disabled");
  },
  setItem() {
    throw new Error("storage disabled");
  },
});

function fakeStorage(seed = {}) {
  const held = new Map(Object.entries(seed));
  return {
    getItem: (key) => (held.has(key) ? held.get(key) : null),
    setItem: (key, value) => held.set(key, String(value)),
    removeItem: (key) => held.delete(key),
    held,
  };
}

describe("the levels a conversation can be read at", () => {
  it("offers three, most detail first", () => {
    expect(DETAIL_LEVELS).toEqual(["all", "messages", "agent"]);
  });

  it("shows everything at 'all'", () => {
    const items = [toolCall(1), committed(2), arrived(3), sentOut(4), agentSaid(5), userSaid(6)];
    expect(itemsAtDetailLevel(items, "all")).toEqual(items);
  });

  it("drops activity and leaves every message at 'all messages'", () => {
    const items = [toolCall(1), committed(2), arrived(3), sentOut(4), agentSaid(5), userSaid(6)];
    expect(itemsAtDetailLevel(items, "messages").map((item) => item.data.sequence)).toEqual([3, 4, 5, 6]);
  });

  it("keeps only this agent's words and the reader's at 'agent only'", () => {
    const items = [toolCall(1), committed(2), arrived(3), sentOut(4), agentSaid(5), userSaid(6)];
    expect(itemsAtDetailLevel(items, "agent").map((item) => item.data.sequence)).toEqual([5, 6]);
  });

  it("reads a message that arrived and one that was sent out as correspondence alike", () => {
    expect(itemIsShownAt(arrived(1), "messages")).toBe(true);
    expect(itemIsShownAt(sentOut(1), "messages")).toBe(true);
    expect(itemIsShownAt(arrived(1), "agent")).toBe(false);
    expect(itemIsShownAt(sentOut(1), "agent")).toBe(false);
  });

  it("reads every kind of event as activity, not only tool calls", () => {
    expect(itemIsShownAt(committed(1), "all")).toBe(true);
    expect(itemIsShownAt(committed(1), "messages")).toBe(false);
    expect(itemIsShownAt(committed(1), "agent")).toBe(false);
  });

  /// The restart notice is an instruction to the agent, the same as anything
  /// the reader types. A level that hid it would hide the thing the agent is
  /// acting on and leave the conversation unreadable — and "Agent only" is the
  /// level a project agent opens at, which is exactly where these arrive.
  it("keeps a notice Build wrote at every level", () => {
    const items = [toolCall(1), arrived(2), sentOut(3), fromBuild(4), userSaid(5)];

    expect(itemsAtDetailLevel(items, "all").map((item) => item.data.sequence)).toContain(4);
    expect(itemsAtDetailLevel(items, "messages").map((item) => item.data.sequence)).toContain(4);
    expect(itemsAtDetailLevel(items, "agent").map((item) => item.data.sequence)).toEqual([4, 5]);
  });

  it("reads a notice Build wrote as this conversation's own, not as correspondence", () => {
    expect(itemIsShownAt(fromBuild(1), "agent")).toBe(true);
    expect(itemIsShownAt(fromBuild(1), "messages")).toBe(true);
    expect(itemIsShownAt(fromBuild(1), "all")).toBe(true);
  });

  it("falls back to showing everything for a level nobody defined", () => {
    const items = [toolCall(1), agentSaid(2)];
    expect(itemsAtDetailLevel(items, "nonsense")).toEqual(items);
    expect(itemsAtDetailLevel(null, "agent")).toEqual([]);
  });

  it("hands 'all' the very items it was given, uncopied", () => {
    const items = [agentSaid(1)];
    expect(itemsAtDetailLevel(items, "all")).toBe(items);
  });
});

describe("the level a conversation opens at", () => {
  it("is the dialogue alone on a project agent's conversation", () => {
    expect(defaultDetailLevel("project")).toBe("agent");
  });

  it("is everything on a workspace, a branch and an issue", () => {
    expect(defaultDetailLevel("workspace")).toBe("all");
    expect(defaultDetailLevel("branch")).toBe("all");
    expect(defaultDetailLevel("issue")).toBe("all");
    expect(defaultDetailLevel(undefined)).toBe("all");
  });
});

describe("remembering a level per conversation", () => {
  it("keys it by the conversation id", () => {
    expect(detailLevelKey("conv-7")).toBe(`${DETAIL_LEVEL_KEY_PREFIX}conv-7`);
  });

  it("reads back what was written, for that conversation only", () => {
    const storage = fakeStorage();
    writeDetailLevel("conv-7", "messages", storage);
    expect(readDetailLevel("conv-7", "project", storage)).toBe("messages");
    expect(readDetailLevel("conv-8", "project", storage)).toBe("agent");
  });

  it("ignores a stored value that is not a level", () => {
    const storage = fakeStorage({ [detailLevelKey("conv-7")]: "everything" });
    expect(readDetailLevel("conv-7", "workspace", storage)).toBe("all");
  });

  it("never writes a level that does not exist", () => {
    const storage = fakeStorage();
    writeDetailLevel("conv-7", "everything", storage);
    expect(storage.held.size).toBe(0);
  });

  it("writes nothing for a conversation with no id yet", () => {
    const storage = fakeStorage();
    writeDetailLevel("", "agent", storage);
    expect(storage.held.size).toBe(0);
    expect(readDetailLevel("", "project", storage)).toBe("agent");
  });

  it("falls back to the kind's default when storage refuses", () => {
    expect(readDetailLevel("conv-7", "project", storageThatRefuses())).toBe("agent");
    expect(() => writeDetailLevel("conv-7", "all", storageThatRefuses())).not.toThrow();
  });
});

describe("the menu rows a level is chosen from", () => {
  it("names one row per level and marks the standing one", () => {
    const options = detailLevelMenuOptions("messages");
    expect(options.map((option) => option.id)).toEqual(["detail:all", "detail:messages", "detail:agent"]);
    expect(options.map((option) => option.label)).toEqual(["All", "All messages", "Agent only"]);
    expect(options.map((option) => option.selected)).toEqual([false, true, false]);
    expect(options.every((option) => option.description)).toBe(true);
  });

  it("reads a level off its own row's id and nothing else", () => {
    expect(detailLevelOfOptionId("detail:agent")).toBe("agent");
    expect(detailLevelOfOptionId("detail:nonsense")).toBe(null);
    expect(detailLevelOfOptionId("shell")).toBe(null);
    expect(detailLevelOfOptionId("")).toBe(null);
    expect(detailLevelOfOptionId(undefined)).toBe(null);
  });
});

describe("what a hidden item does to how far the reader has read", () => {
  const conversation = [agentSaid(10), toolCall(11), toolCall(12), agentSaid(13), toolCall(14)];

  it("absorbs the hidden items that follow the last row the reader reached", () => {
    expect(readThroughHiddenItems(10, conversation, "agent")).toBe(12);
  });

  it("stops at the first drawn row the reader has not reached", () => {
    expect(readThroughHiddenItems(10, [agentSaid(10), toolCall(11), agentSaid(12), toolCall(13)], "agent")).toBe(11);
  });

  it("absorbs a hidden tail, so the badge can clear at the end of a conversation", () => {
    expect(readThroughHiddenItems(13, conversation, "agent")).toBe(14);
  });

  it("moves nothing when nothing has been read", () => {
    expect(readThroughHiddenItems(0, conversation, "agent")).toBe(0);
  });

  it("moves nothing at 'all', where every item was drawn", () => {
    expect(readThroughHiddenItems(10, conversation, "all")).toBe(10);
  });

  it("absorbs correspondence the same way activity is absorbed", () => {
    expect(readThroughHiddenItems(10, [agentSaid(10), arrived(11), sentOut(12), userSaid(13)], "agent")).toBe(12);
  });

  it("leaves items with no sequence out of the reckoning", () => {
    const items = [agentSaid(10), { type: "tool_use", data: { event: "tool_use" } }, toolCall(11)];
    expect(readThroughHiddenItems(10, items, "agent")).toBe(11);
  });

  it("survives an empty conversation", () => {
    expect(readThroughHiddenItems(10, [], "agent")).toBe(10);
    expect(readThroughHiddenItems(10, null, "agent")).toBe(10);
  });
});
