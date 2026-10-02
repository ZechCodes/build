/** @vitest-environment jsdom */
// #65 on the task page: the watch switch, and the read mark.
//
// Automatic read marks are gated on the bridge saying it carries them
// (core/trackerWatch.js), while the switch paints from the cached record.
// This file drives the mark gate rather than the version — its arithmetic is in
// test/trackerWatch.test.js.
//
// The two things that matter here are the ones a fixture cannot get wrong by
// accident: a switch that moves under the finger and goes back on a refusal,
// and a read mark that is sent when the reader has actually seen the end of
// the timeline and is not sent twice for the same point.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, task } from "./trackerWireFixture.js";

let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {},
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] } }),
  bridgeApiVersion: () => "1.7.0",
  watchChanges: (registration) => {
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));

/** The bridge saying this task moved, which is what makes the page re-read. */
const pushed = async () => {
  for (const one of watchers) one.onChanges?.([{ tasks: { task_ids: ["task-1"] } }]);
  await flush();
};

vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

/** The automatic read-mark gate, driven by the case. */
let carries = true;
vi.mock("../src/core/trackerWatch.js", async (original) => ({
  ...(await original()),
  carriesWatching: () => carries,
}));

const PROJECT_KEY = "dev-1|proj-1";
const TIMELINE = [
  event({ id: "te-1", kind: "created", at: "2026-09-21T10:00:00Z" }),
  comment({ id: "tc-1", created_at: "2026-09-21T10:01:00Z", body: "The first word." }),
];

let host, call, page, mountTaskPage, refuses, answer;
const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const button = () => host.querySelector(".rail-watch");
const listed = (method) => call.mock.calls.filter(([name]) => name === method);

const mount = async () => {
  page = mountTaskPage(host, {
    projectId: "proj-1", deviceId: "dev-1", projectKey: PROJECT_KEY, taskId: "task-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ workspaces: [], items: [], projects: [] }),
    navigate: vi.fn(),
  });
  await flush();
  return page;
};

/** The scroller reaching its end, which is the only thing that says the reader
 *  has seen what is at the bottom of it. jsdom lays nothing out, so the three
 *  numbers the page reads are set here. */
const scrollToEnd = async (element = host) => {
  Object.defineProperty(element, "scrollHeight", { value: 1000, configurable: true });
  Object.defineProperty(element, "clientHeight", { value: 400, configurable: true });
  element.scrollTop = 600;
  element.dispatchEvent(new Event("scroll"));
  await flush();
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  carries = true;
  refuses = null;
  watchers = [];
  notifyError.mockClear();
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  const trackerCache = await import("../src/core/trackerCache.js");
  ({ mountTaskPage } = await import("../src/core/trackerTaskPage.js"));
  await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
  answer = {
    task: task({ id: "task-1", number: 65, watched: false, trackers: ["agent-1", "agent-2"] }),
    timeline: TIMELINE,
  };
  call = vi.fn(async (method) => {
    if (refuses === method) throw new Error("the bridge said no");
    return method === "tasks.get" ? answer : {};
  });
});

afterEach(() => page?.dispose());

describe("the watch switch", () => {
  it("draws from the record before the bridge can be asked", async () => {
    carries = false;
    await mount();
    expect(button().getAttribute("aria-pressed")).toBe("false");
  });

  it("says what the record says: not watching, and who else is", async () => {
    await mount();
    expect(button().getAttribute("aria-pressed")).toBe("false");
    expect(button().getAttribute("title")).toBe("Not watching · 2");
  });

  it("…and says it the other way round when the reader watches it", async () => {
    answer.task = task({ id: "task-1", number: 65, watched: true, trackers: ["agent-1"] });
    await mount();
    expect(button().getAttribute("aria-pressed")).toBe("true");
    expect(button().getAttribute("title")).toBe("Watching · 1");
  });

  // Optimistic: the state moves before the verb settles, because a switch that
  // waits out a round trip on a phone reads as broken.
  it("moves under the finger and counts the reader in", async () => {
    await mount();
    button().click();
    expect(button().getAttribute("aria-pressed")).toBe("true");
    expect(button().getAttribute("title")).toBe("Watching · 3");
    await flush();
    expect(listed("tasks.watch")[0][1]).toEqual({ task_id: "task-1" });
  });

  it("asks to stop watching when it was already on", async () => {
    answer.task = task({ id: "task-1", number: 65, watched: true, trackers: ["agent-1"] });
    await mount();
    button().click();
    await flush();
    expect(listed("tasks.unwatch")[0][1]).toEqual({ task_id: "task-1" });
    expect(button().getAttribute("aria-pressed")).toBe("false");
  });

  it("puts itself back and says so when the bridge refuses", async () => {
    refuses = "tasks.watch";
    await mount();
    button().click();
    await flush();
    expect(button().getAttribute("aria-pressed")).toBe("false");
    expect(button().getAttribute("title")).toBe("Not watching · 2");
    expect(notifyError).toHaveBeenCalledWith("Could not change whether you are watching this task", "the bridge said no");
  });

  // The helper draws `pending` as disabled (the shell agent's 4c4fb8a3). This
  // page is the reason that matters: the feed moves on its own and repaints
  // the whole head from state, so markup written mid-flight would otherwise
  // come back offering a press the switch is going to ignore.
  it("offers no press while the verb is in flight, through a repaint", async () => {
    let settle;
    call = vi.fn((method) => (method === "tasks.get"
      ? Promise.resolve(answer)
      : new Promise((done) => { settle = done; })));
    await mount();
    button().click();
    page.feedMoved();
    await flush();
    expect(button().hasAttribute("disabled")).toBe(true);
    settle({});
    await flush();
    expect(button().hasAttribute("disabled")).toBe(false);
  });

  // A press repaints the button and nothing else: a full repaint would take
  // the caret out of a half-written comment.
  it("keeps what the reader is typing", async () => {
    await mount();
    const field = host.querySelector("#task-comment");
    field.value = "half a thought";
    field.focus();
    button().click();
    await flush();
    expect(host.querySelector("#task-comment").value).toBe("half a thought");
    expect(document.activeElement.id).toBe("task-comment");
  });
});

describe("the read mark", () => {
  it("is advanced on open, through the newest row", async () => {
    await mount();
    expect(listed("tasks.read_through")[0][1]).toEqual({ task_id: "task-1", event_id: "tc-1" });
  });

  it("is not sent at all on a bridge that cannot be asked", async () => {
    carries = false;
    await mount();
    expect(listed("tasks.read_through")).toHaveLength(0);
  });

  it("does not send a timeline fallback key as a read mark", async () => {
    answer.timeline = [...TIMELINE, comment({ id: undefined, body: "A legacy comment without an id." })];
    page = mountTaskPage(host, {
      projectId: "proj-1", deviceId: "dev-1", projectKey: PROJECT_KEY, taskId: "task-1",
      callRpc: call,
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ workspaces: [], items: [], projects: [] }),
      navigate: vi.fn(),
    });
    await vi.waitFor(() => expect(host.textContent).toContain("A legacy comment without an id."));
    expect(listed("tasks.read_through")).toHaveLength(0);
    Object.defineProperty(host, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(host, "clientHeight", { value: 400, configurable: true });
    host.scrollTop = 600;
    host.dispatchEvent(new Event("scroll"));
    expect(listed("tasks.read_through")).toHaveLength(0);
  });

  // The mark only moves forward, so the same point is never re-sent: a scroll
  // fires many times and a page that called on each would be a call a frame.
  it("is not re-sent for a point already marked", async () => {
    await mount();
    await scrollToEnd();
    await scrollToEnd();
    expect(listed("tasks.read_through")).toHaveLength(1);
  });

  it("moves again when the timeline has grown and the reader reaches the end", async () => {
    await mount();
    answer = {
      task: answer.task,
      timeline: [...TIMELINE, comment({ id: "tc-2", created_at: "2026-09-21T10:05:00Z", body: "The last word." })],
    };
    await pushed();
    await scrollToEnd();
    const marks = listed("tasks.read_through");
    expect(marks.at(-1)[1]).toEqual({ task_id: "task-1", event_id: "tc-2" });
  });

  // Quiet on both sides: a read mark is housekeeping the reader did not ask
  // for, and a page that toasts about its own notes is worse than one that
  // re-sends on the next scroll.
  it("says nothing to anybody when it is refused", async () => {
    refuses = "tasks.read_through";
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
  });
});
