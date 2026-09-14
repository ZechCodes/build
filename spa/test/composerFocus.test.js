// @vitest-environment jsdom
// A poll must not take the box you are typing in.
//
// Every conversation surface re-renders the whole thread on its tick, and
// writing that string into the container replaces the textarea — which takes
// the caret, the selection and the focus with it. On a touch device that is
// the software keyboard opening and closing again a second and a half later,
// so a message cannot be typed at all. The timeline is what moved, so the
// timeline is what gets swapped: the composer's element is never detached.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const refreshFeed = vi.fn(async () => {});
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  primaryRunIdFor: () => null,
}));
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: async () => {},
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notifySuccess: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));

const { App } = await import("../src/app.js");
const { threadHtml, writeThreadKeepingComposer } = await import("../src/core/thread.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");

const RAIL_COMPOSER = {
  inputId: "railinput",
  sendId: "railsend",
  hintId: "railhint",
  placeholder: "Send a message to this agent…",
  attachable: true,
};

const message = (body) => ({ type: "message", data: { role: "agent", body, sequence: body.length } });

describe("writing a repainted thread", () => {
  let container;

  beforeEach(() => {
    document.body.innerHTML = `<div id="body"></div>`;
    container = document.querySelector("#body");
  });

  it("says the composer is new on the paint that creates it", () => {
    expect(writeThreadKeepingComposer(container, threadHtml({ items: [] }, { composer: RAIL_COMPOSER }))).toBe(true);
    expect(container.querySelector("#railinput")).toBeTruthy();
  });

  it("keeps the very same input element, its words, its caret and its focus", () => {
    writeThreadKeepingComposer(container, threadHtml({ items: [] }, { composer: RAIL_COMPOSER }));
    const input = container.querySelector("#railinput");
    input.focus();
    input.value = "half a thought";
    input.setSelectionRange(4, 9);

    const composerIsNew = writeThreadKeepingComposer(
      container,
      threadHtml({ items: [message("the agent replied")] }, { composer: RAIL_COMPOSER }),
    );

    expect(composerIsNew).toBe(false);
    expect(container.querySelector("#railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
    expect([input.selectionStart, input.selectionEnd]).toEqual([4, 9]);
  });

  it("still swaps the timeline the repaint was for", () => {
    writeThreadKeepingComposer(container, threadHtml({ items: [] }, { composer: RAIL_COMPOSER }));
    expect(container.querySelector(".thread-empty")).toBeTruthy();

    writeThreadKeepingComposer(
      container,
      threadHtml({ items: [message("the agent replied")] }, { composer: RAIL_COMPOSER }),
    );

    expect(container.querySelector(".thread-empty")).toBe(null);
    expect(container.textContent).toContain("the agent replied");
    expect(container.querySelector(".thread-title-text").textContent).toContain("1");
    expect(container.querySelector(".review-thread").classList.contains("is-empty")).toBe(false);
  });

  it("moves the placeholder onto the box that is already there", () => {
    writeThreadKeepingComposer(container, threadHtml({ items: [] }, { composer: RAIL_COMPOSER }));
    const input = container.querySelector("#railinput");

    writeThreadKeepingComposer(
      container,
      threadHtml({ items: [] }, { composer: { ...RAIL_COMPOSER, placeholder: "Send a message to start an agent here…" } }),
    );

    expect(container.querySelector("#railinput")).toBe(input);
    expect(input.placeholder).toBe("Send a message to start an agent here…");
  });

  it("writes the whole section when it is a different composer", () => {
    writeThreadKeepingComposer(container, threadHtml({ items: [] }, { composer: RAIL_COMPOSER }));
    const composerIsNew = writeThreadKeepingComposer(
      container,
      threadHtml({ items: [] }, { composer: { ...RAIL_COMPOSER, inputId: "otherinput" } }),
    );
    expect(composerIsNew).toBe(true);
    expect(container.querySelector("#railinput")).toBe(null);
    expect(container.querySelector("#otherinput")).toBeTruthy();
  });

  // A repaint that says the same thing is not a repaint. Rewriting the timeline
  // anyway collapses a selection the reader was making in a message, and makes
  // every inline image re-fetch itself.
  it("leaves a part of the thread alone when it says exactly what it said", () => {
    const thread = { items: [message("the agent replied")] };
    writeThreadKeepingComposer(container, threadHtml(thread, { composer: RAIL_COMPOSER }));
    const items = container.querySelector(".thread-items");
    const said = items.querySelector(".thread-message");

    writeThreadKeepingComposer(container, threadHtml(thread, { composer: RAIL_COMPOSER }));

    expect(container.querySelector(".thread-items")).toBe(items);
    expect(container.querySelector(".thread-message"), "the timeline was rewritten with itself").toBe(said);
  });

  it("keeps a selection in the timeline through a repaint that changed nothing", () => {
    const thread = { items: [message("the agent replied")] };
    writeThreadKeepingComposer(container, threadHtml(thread, { composer: RAIL_COMPOSER }));
    const body = container.querySelector(".thread-body");
    const range = document.createRange();
    range.selectNodeContents(body);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);

    writeThreadKeepingComposer(container, threadHtml(thread, { composer: RAIL_COMPOSER }));

    expect(window.getSelection().toString()).toContain("the agent replied");
    window.getSelection().removeAllRanges();
  });

  it("writes the whole section for a thread that has no composer at all", () => {
    expect(writeThreadKeepingComposer(container, threadHtml({ items: [] }))).toBe(true);
    expect(container.querySelector(".review-thread")).toBeTruthy();
  });
});

describe("the rail's poll", () => {
  let rail = null;
  let payload = null;

  const flush = async () => {
    for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
  };

  const branchRow = (items) => ({
    kind: "branch",
    project_id: "p1",
    branch: "build/login",
    run_id: "run-3",
    worktree_id: "wt-3",
    agents: [{ id: "ag-1", ordinal: 1, provider: "claude_adk", state: "live", unread_count: 0, working: false }],
    run: { run_id: "run-3", thread: { items, sessions: [] } },
  });

  beforeEach(async () => {
    document.body.innerHTML = bodyHtml;
    localStorage.clear();
    resetAgentRailMemory();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    payload = branchRow([]);
    App.call = vi.fn(async (method) => (method === "branch.get" ? payload : {}));
    rail = mountAgentRail(document.getElementById("agent-rail"), {
      kind: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "build/login",
      call: (method, params) => App.call(method, params),
    });
    await flush();
  });

  afterEach(() => {
    if (rail) rail.dispose();
    rail = null;
    vi.useRealTimers();
  });

  it("leaves the box you are typing in exactly where it was", async () => {
    const input = document.getElementById("railinput");
    input.focus();
    input.value = "please look at the";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.setSelectionRange(7, 7);

    payload = branchRow([message("the agent replied")]);
    vi.advanceTimersByTime(2000);
    await flush();

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("please look at the");
    expect([input.selectionStart, input.selectionEnd]).toEqual([7, 7]);
    expect(document.getElementById("rail-body").textContent).toContain("the agent replied");
  });

  it("still sends what the box holds after a repaint under it", async () => {
    const input = document.getElementById("railinput");
    input.focus();
    input.value = "ship it";
    input.dispatchEvent(new Event("input", { bubbles: true }));

    payload = branchRow([message("the agent replied")]);
    vi.advanceTimersByTime(2000);
    await flush();

    document.getElementById("railsend").click();
    await flush();

    const post = App.call.mock.calls.find(([method]) => method === "thread.post");
    expect(post[1]).toMatchObject({ entity_id: "run-3", agent_id: "ag-1", body: "ship it" });
  });
});

// A killed harness does not make the work item go away, but it does make the
// daemon's row for it fall back to a source that knows nothing about agents —
// the bare checkout under the branch. So the rail reads a run and an agent on
// one tick and neither on the next, over and over, for as long as the agent is
// dead. The panel is keyed on which agent is open, so every one of those ticks
// threw the whole panel away and built a new one: a new head, a new timeline,
// and a new textarea. On a phone that is the keyboard opening and shutting
// every 1.6 seconds, which is a message that cannot be typed at all.
describe("a work item that keeps losing its agent", () => {
  let rail = null;
  let payload = null;

  const flush = async () => {
    for (let i = 0; i < 8; i++) await new Promise((done) => setTimeout(done, 0));
  };

  const withAgent = () => ({
    kind: "branch",
    project_id: "p1",
    branch: "build/login",
    run_id: "run-3",
    worktree_id: "wt-3",
    agents: [{ id: "ag-1", ordinal: 1, provider: "claude_adk", state: "exited", unread_count: 0, working: false }],
    run: { run_id: "run-3", thread: { items: [], sessions: [] } },
  });

  // The same branch, read off the checkout instead: no run, no conversation,
  // no agents.
  const bareCheckout = () => ({
    kind: "branch",
    project_id: "p1",
    branch: "build/login",
    run_id: null,
    worktree_id: "wt-3",
    agents: [],
    run: null,
  });

  const mount = async () => {
    payload = withAgent();
    App.call = vi.fn(async (method) => (method === "branch.get" ? payload : {}));
    rail = mountAgentRail(document.getElementById("agent-rail"), {
      kind: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "build/login",
      call: (method, params) => App.call(method, params),
    });
    await flush();
  };

  const typeInto = () => {
    const input = document.getElementById("railinput");
    input.focus();
    input.value = "half a thought";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return input;
  };

  const flap = async (rowAt, ticks = 6) => {
    for (let tick = 0; tick < ticks; tick += 1) {
      payload = rowAt(tick);
      vi.advanceTimersByTime(1700);
      await flush();
    }
  };

  beforeEach(() => {
    document.body.innerHTML = bodyHtml;
    localStorage.clear();
    resetAgentRailMemory();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  });

  afterEach(() => {
    if (rail) rail.dispose();
    rail = null;
    vi.useRealTimers();
  });

  it("keeps the very same box through a tick that names no agent at all", async () => {
    await mount();
    const input = typeInto();

    await flap((tick) => (tick % 2 ? { ...withAgent(), agents: [] } : withAgent()));

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
  });

  it("keeps it through the run coming and going with the agent", async () => {
    await mount();
    const input = typeInto();

    await flap((tick) => (tick % 2 ? bareCheckout() : withAgent()));

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
  });

  // The rail has no media path of its own — the phone lays the panel over the
  // work in CSS and changes nothing about what paints — so the guard has to
  // hold at a phone's width for the same reason it holds at a desk's.
  it("keeps it at a phone's width, where losing it costs the keyboard", async () => {
    Object.defineProperty(window, "innerWidth", { value: 390, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 844, configurable: true });
    await mount();
    const input = typeInto();

    await flap((tick) => (tick % 2 ? bareCheckout() : withAgent()), 12);

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
  });

  // A daemon that knows about agents refuses the agent id the rail names when
  // the row it resolved this tick has no run behind it — so the rail hears the
  // hiccup twice over: a refusal, and then the agent-less row itself on the
  // unscoped ask that follows. Both are the same non-answer.
  it("keeps the box when the daemon refuses the agent it asked about", async () => {
    let tick = 0;
    App.call = vi.fn(async (method, params) => {
      if (method !== "branch.get") return {};
      // Two ticks off the bare checkout for every one that resolves the run.
      const bare = tick++ % 3 !== 2;
      if (!bare) return withAgent();
      if (params.agent_id) throw new Error(`unknown agent_id: ${params.agent_id}`);
      return bareCheckout();
    });
    rail = mountAgentRail(document.getElementById("agent-rail"), {
      kind: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "build/login",
      call: (method, params) => App.call(method, params),
    });
    await flush();
    // The first read lands on a bare tick; the run resolves on the third.
    await flap(() => null, 3);
    const input = typeInto();

    await flap(() => null, 12);

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
    expect(document.querySelector(".rail-who").textContent).toBe("Claude Code 1");
  });

  // The same tick that rebuilt the panel also rewrote the strip and the head:
  // the agent's bubble became a ghost and the name above the conversation
  // became "New agent", a second and a half at a time.
  it("keeps saying whose conversation is open", async () => {
    await mount();

    await flap((tick) => (tick % 2 ? bareCheckout() : withAgent()));

    const strip = document.querySelector(".rail-strip");
    expect([...strip.querySelectorAll(".rail-bubble")].map((bubble) => bubble.dataset.bubble))
      .toEqual(["agent", "add"]);
    expect(document.querySelector(".rail-who").textContent).toBe("Claude Code 1");
  });

  // The guard is a hiccup filter, not a freeze: a branch whose run really has
  // gone (finished, abandoned) keeps answering without agents, and the rail
  // falls back to the ghost that starts a new one — one tick later than it used
  // to, and no later than that.
  it("believes a run that is really gone the second time it says so", async () => {
    await mount();

    await flap(() => bareCheckout(), 2);

    const strip = document.querySelector(".rail-strip");
    expect([...strip.querySelectorAll(".rail-bubble")].map((bubble) => bubble.dataset.bubble))
      .toEqual(["ghost"]);
    expect(document.getElementById("railinput").placeholder)
      .toBe("Send a message to start an agent here…");
  });
});
