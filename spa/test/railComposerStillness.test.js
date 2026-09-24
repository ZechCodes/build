// @vitest-environment jsdom
// Nothing the rail repaints may touch the box somebody is typing in (#138).
//
// On a phone, the maintainer's swipes died mid-word and key taps went missing
// while an agent worked. The textarea itself was never replaced
// (composerFocus.test.js holds that), but every push rewrote the composer row
// and the rail's attributes, most of them with the value they already had:
// fifty writes a second, two of them on the panel the textarea sits in, plus
// two style writes on the textarea and a new send button for every key.
//
// So nothing an agent does writes into the composer row: its Working clock and
// its context gauge tick in the footer beside the row, never inside it. And no
// attribute anywhere in the rail is set to the value it already holds.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
}));
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: async () => {},
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notifySuccess: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));

const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { writeUserSession } = await import("../src/core/userSessionCache.js");
const { writeRailWorkItem } = await import("./railCacheFixture.js");

const flush = async () => {
  for (let i = 0; i < 14; i++) await new Promise((done) => setTimeout(done, 0));
};

const said = (role, body, sequence) => ({ type: "message", data: { role, body, sequence } });

/** The branch's row as a push leaves it: one agent, how far into its window
 *  its last turn went, whether it is working and since when, and the
 *  conversation so far. */
const branchRow = ({ tokens = 100000, working = false, since = null, items = [] } = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [{
    id: "ag-1",
    ordinal: 1,
    provider: "claude_adk",
    state: "live",
    unread_count: 0,
    working,
    working_time: working && since ? { since } : null,
    topic: "Fix login redirect",
    last_context_tokens: tokens,
  }],
  run: { run_id: "run-3", thread: { items, sessions: [] } },
});

const conversation = [
  said("user", "look at the login redirect", 1),
  said("agent", "reading the handler", 2),
  said("user", "and the cookie path", 3),
];

/** Every mutation under the rail from here on, with the value each attribute
 *  held before it was written. */
function watchRail(rail) {
  const records = [];
  const observer = new MutationObserver((batch) => records.push(...batch));
  observer.observe(rail, { subtree: true, childList: true, attributes: true, attributeOldValue: true, characterData: true });
  return {
    take: () => {
      records.push(...observer.takeRecords());
      observer.disconnect();
      return records;
    },
  };
}

const describeTarget = (record) => {
  const node = record.target.nodeType === 1 ? record.target : record.target.parentElement;
  const name = node.id ? `#${node.id}` : `${node.tagName.toLowerCase()}.${[...node.classList].join(".")}`;
  return `${record.type} ${name}${record.attributeName ? `@${record.attributeName}` : ""}`;
};

/** The attribute writes that set exactly what the attribute already said. A
 *  record carries only the value before it, so the value a write left is the
 *  next write's "before" — or, for the last one, what the attribute says now. */
function rewritesOfItself(records) {
  const writes = records.filter((record) => record.type === "attributes");
  return writes.filter((record, index) => {
    const next = writes.slice(index + 1).find((later) =>
      later.target === record.target && later.attributeName === record.attributeName);
    const left = next ? next.oldValue : record.target.getAttribute(record.attributeName);
    return record.oldValue === left;
  });
}

const atWidth = (width, height) => {
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true });
  Object.defineProperty(window, "innerHeight", { value: height, configurable: true });
};

describe("the rail composer while an agent works", () => {
  let rail = null;

  const push = async (row) => {
    await writeRailWorkItem(row);
    await flush();
  };

  /** The panel is a card over the work at a phone's width, shut until its
   *  bubble is pressed: the layout the maintainer was typing in. */
  const mountOnAPhone = async () => {
    atWidth(390, 844);
    await writeRailWorkItem(branchRow({ items: conversation }));
    rail = mountAgentRail(document.getElementById("agent-rail"), {
      kind: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "build/login",
      call: async () => ({}),
    });
    await flush();
    document.querySelector(".rail-bubble").click();
    await flush();
  };

  const typeInto = () => {
    const input = document.getElementById("railinput");
    input.focus();
    for (const word of ["half", " a", " thought"]) {
      input.value += word;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    input.setSelectionRange(5, 5);
    return input;
  };

  const clock = () => document.querySelector(".rail-status-text")?.textContent ?? "";

  /** Seconds passing on the rail's own clock. */
  const tick = async (ms) => {
    vi.advanceTimersByTime(ms);
    await flush();
  };

  /** Everything that moves while an agent works and the reader types: its
   *  turn starting, its Working clock running, the gauge climbing, its words
   *  arriving, the reader's own arrival recorded, the conversation scrolled,
   *  and the turn ending. Says what the clock read as the turn began and
   *  after it had run. */
  const workAround = async () => {
    const since = new Date(Date.now() - 5000).toISOString();
    const working = { working: true, since };
    await push(branchRow({ items: conversation, ...working }));
    const began = clock();
    await tick(3000);
    await push(branchRow({ items: conversation, ...working, tokens: 120000 }));
    await push(branchRow({ items: [...conversation, said("agent", "found it", 4)], ...working, tokens: 120000 }));
    await tick(2000);
    const ran = clock();
    await writeUserSession("dev-1", { user_session: { gap_ms: 0, session_started_ms: Date.now(), now_ms: Date.now() } });
    await flush();
    document.getElementById("rail-body").dispatchEvent(new Event("scroll"));
    await flush();
    await push(branchRow({ items: [...conversation, said("agent", "found it", 4)], working: false, tokens: 140000 }));
    return { began, ran };
  };

  beforeEach(async () => {
    document.body.innerHTML = bodyHtml;
    localStorage.clear();
    await wipeCache();
    resetAgentRailMemory();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  });

  afterEach(() => {
    if (rail) rail.dispose();
    rail = null;
    atWidth(1024, 768);
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("keeps the same box, its words, its caret and its focus", async () => {
    await mountOnAPhone();
    const input = typeInto();

    await workAround();

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
    expect([input.selectionStart, input.selectionEnd]).toEqual([5, 5]);
  });

  it("writes nothing in the composer row while the clock runs and the gauge climbs", async () => {
    await mountOnAPhone();
    typeInto();
    const composer = document.getElementById("rail-composer");
    const gauge = document.getElementById("railinputgauge");
    const watch = watchRail(document.getElementById("agent-rail"));

    const { began, ran } = await workAround();

    const records = watch.take();
    const inComposer = records.filter((record) => composer.contains(record.target));
    expect(inComposer.map(describeTarget)).toEqual([]);
    // Both did move, beside the row rather than in it.
    expect(began).not.toBe("");
    expect(ran).not.toBe(began);
    expect(gauge.textContent).toBe("14%");
    expect(composer.contains(gauge)).toBe(false);
    expect(composer.contains(document.querySelector(".rail-status-text"))).toBe(false);
  });

  it("leaves the clock's text alone on a tick that reads the same", async () => {
    // The rail's clock ticks every second; with the time standing still each
    // tick reads what the last one did.
    vi.useRealTimers();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.spyOn(Date, "now").mockReturnValue(Date.now());
    await mountOnAPhone();
    typeInto();
    await push(branchRow({ items: conversation, working: true, since: new Date(Date.now() - 5000).toISOString() }));
    const text = document.querySelector(".rail-status-text");
    expect(text.textContent).not.toBe("");
    const writes = [];
    const observer = new MutationObserver((batch) => writes.push(...batch));
    observer.observe(text, { childList: true, characterData: true, subtree: true });

    vi.advanceTimersByTime(10_000);
    await flush();
    writes.push(...observer.takeRecords());
    observer.disconnect();

    expect(writes.map(describeTarget)).toEqual([]);
  });

  it("sets no attribute in the rail to the value it already had", async () => {
    await mountOnAPhone();
    typeInto();
    const watch = watchRail(document.getElementById("agent-rail"));

    await workAround();

    expect(rewritesOfItself(watch.take()).map(describeTarget)).toEqual([]);
  });

  it("writes nothing on the box itself as the reader types along a line", async () => {
    await mountOnAPhone();
    const input = typeInto();
    const writes = [];
    const observer = new MutationObserver((batch) => writes.push(...batch));
    observer.observe(input, { attributes: true });

    for (const letter of "and more") {
      input.value += letter;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    writes.push(...observer.takeRecords());
    observer.disconnect();

    expect(writes.map(describeTarget)).toEqual([]);
  });

  it("rebuilds the send control only when its shape changes", async () => {
    await mountOnAPhone();
    const input = document.getElementById("railinput");
    input.focus();
    const control = document.getElementById("railinputsendcontrol");
    const send = document.getElementById("railsend");

    for (const letter of "ship it") {
      input.value += letter;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }

    expect(control.querySelector("button")).toBe(send);
  });
});
