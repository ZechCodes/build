/** @vitest-environment jsdom */
// #65 on the issue page: the watch switch, and the read mark.
//
// Both call verbs #64 is still building, so both are gated on the bridge
// saying it carries them (core/trackerWatch.js) and this file drives the gate
// rather than the version — the gate's own arithmetic is held in
// test/trackerWatch.test.js.
//
// The two things that matter here are the ones a fixture cannot get wrong by
// accident: a switch that moves under the finger and goes back on a refusal,
// and a read mark that is sent when the reader has actually seen the end of
// the timeline and is not sent twice for the same point.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, issue } from "./trackerWireFixture.js";

let watchers = [];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["issues"] } }),
  bridgeApiVersion: () => "1.7.0",
  watchChanges: (registration) => {
    watchers.push(registration);
    return { dispose: () => {} };
  },
}));

/** The bridge saying this issue moved, which is what makes the page re-read. */
const pushed = async () => {
  for (const one of watchers) one.onChanges?.([{ issues: { issue_ids: ["issue-1"] } }]);
  await flush();
};

vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

/** The gate, driven by the case. The bridge that carries these verbs does not
 *  exist yet — #64 is building it — which is the whole reason it is a gate. */
let carries = true;
vi.mock("../src/core/trackerWatch.js", async (original) => ({
  ...(await original()),
  carriesWatch: () => carries,
}));

const PROJECT_KEY = "dev-1|proj-1";
const TIMELINE = [
  event({ id: "ie-1", kind: "created", at: "2026-09-21T10:00:00Z" }),
  comment({ id: "ic-1", created_at: "2026-09-21T10:01:00Z", body: "The first word." }),
];

let host, call, page, mountIssuePage, refuses, answer;
const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const button = () => host.querySelector(".rail-watch");
const listed = (method) => call.mock.calls.filter(([name]) => name === method);

const mount = async () => {
  page = mountIssuePage(host, {
    projectId: "proj-1", deviceId: "dev-1", projectKey: PROJECT_KEY, issueId: "issue-1",
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
  ({ mountIssuePage } = await import("../src/core/trackerIssuePage.js"));
  await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
  answer = {
    issue: issue({ id: "issue-1", number: 65, watched: false, trackers: ["agent-1", "agent-2"] }),
    timeline: TIMELINE,
  };
  call = vi.fn(async (method) => {
    if (refuses === method) throw new Error("the bridge said no");
    return method === "issues.get" ? answer : {};
  });
});

afterEach(() => page?.dispose());

describe("the watch switch", () => {
  it("is not drawn at all on a bridge that cannot be asked", async () => {
    carries = false;
    await mount();
    expect(button()).toBeNull();
  });

  it("says what the record says: not watching, and who else is", async () => {
    await mount();
    expect(button().getAttribute("aria-pressed")).toBe("false");
    expect(button().getAttribute("title")).toBe("Not watching · 2");
  });

  it("…and says it the other way round when the reader watches it", async () => {
    answer.issue = issue({ id: "issue-1", number: 65, watched: true, trackers: ["agent-1"] });
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
    expect(listed("issues.watch")[0][1]).toEqual({ issue_id: "issue-1" });
  });

  it("asks to stop watching when it was already on", async () => {
    answer.issue = issue({ id: "issue-1", number: 65, watched: true, trackers: ["agent-1"] });
    await mount();
    button().click();
    await flush();
    expect(listed("issues.unwatch")[0][1]).toEqual({ issue_id: "issue-1" });
    expect(button().getAttribute("aria-pressed")).toBe("false");
  });

  it("puts itself back and says so when the bridge refuses", async () => {
    refuses = "issues.watch";
    await mount();
    button().click();
    await flush();
    expect(button().getAttribute("aria-pressed")).toBe("false");
    expect(button().getAttribute("title")).toBe("Not watching · 2");
    expect(notifyError).toHaveBeenCalledWith("Could not change whether you are watching this issue");
  });

  // A press repaints the button and nothing else: a full repaint would take
  // the caret out of a half-written comment.
  it("keeps what the reader is typing", async () => {
    await mount();
    const field = host.querySelector("#issue-comment");
    field.value = "half a thought";
    field.focus();
    button().click();
    await flush();
    expect(host.querySelector("#issue-comment").value).toBe("half a thought");
    expect(document.activeElement.id).toBe("issue-comment");
  });
});

describe("the read mark", () => {
  it("is advanced on open, through the newest row", async () => {
    await mount();
    expect(listed("issues.read_through")[0][1]).toEqual({ issue_id: "issue-1", event_id: "ic-1" });
  });

  it("is not sent at all on a bridge that cannot be asked", async () => {
    carries = false;
    await mount();
    expect(listed("issues.read_through")).toHaveLength(0);
  });

  // The mark only moves forward, so the same point is never re-sent: a scroll
  // fires many times and a page that called on each would be a call a frame.
  it("is not re-sent for a point already marked", async () => {
    await mount();
    await scrollToEnd();
    await scrollToEnd();
    expect(listed("issues.read_through")).toHaveLength(1);
  });

  it("moves again when the timeline has grown and the reader reaches the end", async () => {
    await mount();
    answer = {
      issue: answer.issue,
      timeline: [...TIMELINE, comment({ id: "ic-2", created_at: "2026-09-21T10:05:00Z", body: "The last word." })],
    };
    await pushed();
    await scrollToEnd();
    const marks = listed("issues.read_through");
    expect(marks.at(-1)[1]).toEqual({ issue_id: "issue-1", event_id: "ic-2" });
  });

  // Quiet on both sides: a read mark is housekeeping the reader did not ask
  // for, and a page that toasts about its own notes is worse than one that
  // re-sends on the next scroll.
  it("says nothing to anybody when it is refused", async () => {
    refuses = "issues.read_through";
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
  });
});
