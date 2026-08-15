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
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notify: () => {} }));
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
    agents: [{ id: "ag-1", ordinal: 1, provider: "claude", state: "live", unread_count: 0, working: false }],
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
      projectId: "p1",
      branch: "build/login",
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
