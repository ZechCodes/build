// @vitest-environment jsdom
// A tick that says nothing must do nothing.
//
// The conversation re-renders every 1.6 seconds. When the poll resolves the
// same messages the repaint has nothing to say, and every byte it writes anyway
// costs the reader: replacing a node under a scrolled viewport hands the
// browser's scroll anchoring a moving target, an inline picture that loses its
// `src` re-lays-out at zero height, and assigning scrollTop at all cancels a
// finger's momentum on iOS. Which is the bug this file pins: scrolling DOWN a
// thread on a phone snapped back to the same place a second and a half at a
// time. So the rule is identity — same conversation, same nodes, and the
// scroller untouched.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  paintThreadKeepingPlace,
  threadHtml,
  wireThreadAttachments,
  writeThreadKeepingComposer,
} from "../src/core/thread.js";

const RAIL_COMPOSER = {
  inputId: "railinput",
  sendId: "railsend",
  hintId: "railhint",
  placeholder: "Send a message to this agent…",
  attachable: true,
};

/** jsdom has no layout, so the scroller states its own geometry — and counts
 *  every write to scrollTop, because "did not move the reader" is not the same
 *  claim as "wrote the number it already held". */
function scroller({ scrollHeight = 1000, clientHeight = 300 } = {}) {
  const element = document.createElement("div");
  const writes = [];
  let top = 0;
  Object.defineProperty(element, "scrollHeight", { get: () => scrollHeight, configurable: true });
  Object.defineProperty(element, "clientHeight", { get: () => clientHeight, configurable: true });
  Object.defineProperty(element, "scrollTop", {
    get: () => top,
    set: (value) => {
      writes.push(value);
      top = value;
    },
    configurable: true,
  });
  document.body.appendChild(element);
  return { element, writes, place: (value) => { top = value; } };
}

/** Everything the DOM moved while `act` ran. */
function mutationsDuring(root, act) {
  const observer = new MutationObserver(() => {});
  observer.observe(root, { childList: true, subtree: true, attributes: true, characterData: true });
  try {
    act();
    return observer.takeRecords();
  } finally {
    observer.disconnect();
  }
}

const describeRecord = (record) =>
  `${record.type}${record.attributeName ? ` ${record.attributeName}` : ""} on ${record.target.nodeName}.${record.target.className || ""}`;

const message = (body, extra = {}) => ({
  type: "message",
  data: { role: "agent", body, created_at: "2026-08-13T12:00:00.000Z", sequence: 3, ...extra },
});

const paintInto = (host, thread) => () => {
  const html = threadHtml(thread, { composer: RAIL_COMPOSER });
  writeThreadKeepingComposer(host, html);
};

describe("a repaint that resolves the same conversation", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("moves nothing in the timeline at all", () => {
    const thread = { items: [message("the agent replied"), message("and again", { sequence: 4 })] };
    const { element } = scroller();
    paintThreadKeepingPlace(element, paintInto(element, thread));

    const records = mutationsDuring(element, () =>
      paintThreadKeepingPlace(element, paintInto(element, thread)),
    );

    expect(records.map(describeRecord)).toEqual([]);
  });

  it("leaves scrollTop alone for a reader who scrolled up", () => {
    const thread = { items: [message("the agent replied")] };
    const { element, writes, place } = scroller();
    paintThreadKeepingPlace(element, paintInto(element, thread));
    place(240);
    writes.length = 0;

    paintThreadKeepingPlace(element, paintInto(element, thread));

    expect(writes).toEqual([]);
    expect(element.scrollTop).toBe(240);
  });

  it("leaves scrollTop alone for a reader sitting at the end", () => {
    const thread = { items: [message("the agent replied")] };
    const { element, writes, place } = scroller();
    paintThreadKeepingPlace(element, paintInto(element, thread));
    place(700);
    writes.length = 0;

    paintThreadKeepingPlace(element, paintInto(element, thread));

    expect(writes).toEqual([]);
    expect(element.scrollTop).toBe(700);
  });

  it("keeps the picture the browser already fetched", async () => {
    const thread = {
      items: [
        message("here it is", {
          role: "user",
          attachments: [{ path: "shots/one.png", name: "one.png", mime: "image/png", size: 12 }],
        }),
      ],
    };
    const { element } = scroller();
    paintThreadKeepingPlace(element, paintInto(element, thread));
    wireThreadAttachments(element, async () => ({ mime: "image/png", content_b64: "AAAA" }));
    await Promise.resolve();
    await Promise.resolve();
    const picture = element.querySelector("img.thread-attachment-image");
    expect(picture.getAttribute("src")).toBe("data:image/png;base64,AAAA");

    const records = mutationsDuring(element, () => {
      paintThreadKeepingPlace(element, paintInto(element, thread));
      wireThreadAttachments(element, async () => ({ mime: "image/png", content_b64: "AAAA" }));
    });

    expect(records.map(describeRecord)).toEqual([]);
    expect(element.querySelector("img.thread-attachment-image")).toBe(picture);
    expect(picture.getAttribute("src")).toBe("data:image/png;base64,AAAA");
  });

  // A picture that will not come is a state of the conversation too, so the
  // render says it. Otherwise the repaint would take the notice off and ask for
  // the bytes again, every tick, forever.
  it("keeps saying a picture is unavailable once it is", async () => {
    const thread = {
      items: [
        message("here it is", {
          role: "user",
          sequence: 9,
          attachments: [{ path: "shots/gone.png", name: "gone.png", mime: "image/png", size: 12 }],
        }),
      ],
    };
    const { element } = scroller();
    let asked = 0;
    const refuse = async () => {
      asked += 1;
      throw new Error("no such attachment");
    };
    paintThreadKeepingPlace(element, paintInto(element, thread));
    wireThreadAttachments(element, refuse);
    await Promise.resolve();
    await Promise.resolve();
    const figure = element.querySelector(".thread-attachment-figure");
    expect(figure.classList.contains("unavailable")).toBe(true);

    const records = mutationsDuring(element, () => {
      paintThreadKeepingPlace(element, paintInto(element, thread));
      wireThreadAttachments(element, refuse);
    });

    expect(records.map(describeRecord)).toEqual([]);
    expect(element.querySelector(".thread-attachment-figure")).toBe(figure);
    expect(asked).toBe(1);
  });
});

describe("a repaint that has something new to say", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  // An agent that is working streams into its newest message. Only that message
  // changed, so only that message may move — the ones above it are what the
  // reader is looking at, and replacing them is what dragged the scroll.
  it("keeps a mid-scroll reader's place while the newest message grows", () => {
    const growing = (words) => ({
      items: [message("the first thing"), message(words, { sequence: 4 })],
    });
    const { element, writes, place } = scroller();
    paintThreadKeepingPlace(element, paintInto(element, growing("working")));
    place(240);
    writes.length = 0;
    const first = element.querySelector(".thread-message");

    for (const words of ["working on", "working on it", "working on it now"]) {
      paintThreadKeepingPlace(element, paintInto(element, growing(words)));
      expect(element.scrollTop).toBe(240);
    }

    expect(element.querySelector(".thread-message")).toBe(first);
    expect(element.textContent).toContain("working on it now");
    expect(writes.every((value) => value === 240)).toBe(true);
  });

  it("rewrites a message's words without replacing the message", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    writeThreadKeepingComposer(host, threadHtml({ items: [message("half")] }, { composer: RAIL_COMPOSER }));
    const said = host.querySelector(".thread-message");
    const body = host.querySelector(".thread-body");

    writeThreadKeepingComposer(host, threadHtml({ items: [message("half a thought")] }, { composer: RAIL_COMPOSER }));

    expect(host.querySelector(".thread-message")).toBe(said);
    expect(host.querySelector(".thread-body")).toBe(body);
    expect(body.textContent).toContain("half a thought");
  });

  // The only thing on a quiet thread that really does change on its own is how
  // long ago something was said. That is a word, so a word is what changes.
  it("re-words a time without replacing the element holding it", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-08-13T12:00:10.000Z"));
      const thread = { items: [message("the agent replied")] };
      const host = document.createElement("div");
      document.body.appendChild(host);
      writeThreadKeepingComposer(host, threadHtml(thread, { composer: RAIL_COMPOSER }));
      const said = host.querySelector(".thread-message");
      const time = host.querySelector("time");
      expect(time.textContent).toBe("Just now");

      vi.setSystemTime(new Date("2026-08-13T12:02:00.000Z"));
      const records = mutationsDuring(host, () =>
        writeThreadKeepingComposer(host, threadHtml(thread, { composer: RAIL_COMPOSER })),
      );

      expect(host.querySelector(".thread-message")).toBe(said);
      expect(host.querySelector("time")).toBe(time);
      expect(time.textContent).toBe("2 minutes ago");
      expect(records.map((record) => record.type)).toEqual(["characterData"]);
    } finally {
      vi.useRealTimers();
    }
  });
});

// The panel over the work is where a phone reads a conversation, and the rail
// is the only thing that paints one. Three ticks of the same answer must leave
// its scroller exactly as it found it.
describe("the rail's poll on an unchanged conversation", () => {
  const bodyHtml = readFileSync(resolve("index.html"), "utf8")
    .match(/<body>([\s\S]*)<\/body>/)[1];

  let App = null;
  let mountAgentRail = null;
  let resetAgentRailMemory = null;
  let rail = null;

  const flush = async () => {
    for (let i = 0; i < 8; i++) await new Promise((done) => setTimeout(done, 0));
  };

  const now = "2026-08-13T12:00:00.000Z";
  const row = () => ({
    kind: "branch",
    project_id: "p1",
    branch: "build/login",
    run_id: "run-3",
    worktree_id: "wt-3",
    agents: [{ id: "ag-1", ordinal: 1, provider: "claude", state: "live", unread_count: 0, working: true }],
    run: {
      run_id: "run-3",
      thread: {
        sessions: [{ provider: "claude" }],
        thread_total: 2,
        items: [
          { type: "event", data: { event: "session_started", created_at: now, sequence: 1 } },
          { type: "message", data: { role: "agent", body: "on it", created_at: now, sequence: 2 } },
        ],
      },
    },
  });

  let frame = null;

  beforeEach(async () => {
    // The paint that opens the panel re-pins the bottom a frame later, on
    // purpose. Running that frame at once puts it before the counter below, so
    // what is counted is the ticks that follow the open — which is the trial.
    frame = globalThis.requestAnimationFrame;
    globalThis.requestAnimationFrame = (callback) => {
      callback();
      return 0;
    };
    vi.resetModules();
    vi.doMock("../src/core/taskFeed.js", () => ({
      subscribeFeed: () => () => {},
      startFeed: () => {},
      stopFeed: () => {},
      refreshFeed: async () => {},
      primaryRunIdFor: () => null,
    }));
    vi.doMock("../src/core/inboxView.js", () => ({
      markSeen: async () => {},
      noteSelfAction: async () => {},
      mountInboxList: () => {},
      inboxListRouteChanged: () => {},
    }));
    vi.doMock("../src/core/notify.js", () => ({ notifyError: () => {}, notify: () => {} }));
    vi.doMock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));
    ({ App } = await import("../src/app.js"));
    ({ mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js"));

    document.body.innerHTML = bodyHtml;
    localStorage.clear();
    resetAgentRailMemory();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    App.call = vi.fn(async (method) => (method === "branch.get" ? row() : {}));
    rail = mountAgentRail(document.getElementById("agent-rail"), {
      kind: "branch",
      projectId: "p1",
      branch: "build/login",
    });
    await flush();
  });

  afterEach(() => {
    if (rail) rail.dispose();
    rail = null;
    globalThis.requestAnimationFrame = frame;
    vi.useRealTimers();
    vi.doUnmock("../src/core/taskFeed.js");
    vi.doUnmock("../src/core/inboxView.js");
    vi.doUnmock("../src/core/notify.js");
    vi.doUnmock("../src/core/surfaceTabs.js");
  });

  it("changes no node and writes no scroll position", async () => {
    const body = document.getElementById("rail-body");
    const writes = [];
    let top = 180;
    Object.defineProperty(body, "scrollHeight", { get: () => 1000, configurable: true });
    Object.defineProperty(body, "clientHeight", { get: () => 300, configurable: true });
    Object.defineProperty(body, "scrollTop", {
      get: () => top,
      set: (value) => {
        writes.push(value);
        top = value;
      },
      configurable: true,
    });
    const records = [];
    const observer = new MutationObserver((list) => records.push(...list));
    observer.observe(body, { childList: true, subtree: true, attributes: true, characterData: true });

    for (let tick = 0; tick < 3; tick += 1) {
      vi.advanceTimersByTime(1700);
      await flush();
    }
    records.push(...observer.takeRecords());
    observer.disconnect();

    expect(records.map(describeRecord)).toEqual([]);
    expect(writes).toEqual([]);
    expect(top).toBe(180);
  });
});

describe("the conversation's scroller", () => {
  const shell = readFileSync(resolve("src/styles/shell.css"), "utf8");

  // Chrome adjusts scrollTop to keep whatever it anchored on still, which is
  // exactly the wrong instinct under a timeline that grows at the bottom.
  // Safari ignores the property, and nothing here relies on it: the repaint
  // above is the fix, and this is the belt beside it.
  it("opts out of the browser's scroll anchoring", () => {
    const rule = shell.match(/\.rail-body \{([^}]*)\}/);
    expect(rule, ".rail-body has no rule to opt out in").toBeTruthy();
    expect(rule[1].replace(/\s/g, "")).toContain("overflow-anchor:none");
  });
});
