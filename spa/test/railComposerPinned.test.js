// @vitest-environment jsdom
// Where the box you write in lives.
//
// It used to be the last thing in the timeline, so it scrolled away with the
// conversation: reading back through a thread took the composer off screen, and
// a message typed into a grown box pushed its own bottom edge past the panel.
//
// The panel is a column now — head, thread, composer — and only the middle row
// scrolls. The composer is a SIBLING of the scroller, pinned to the panel's
// bottom edge, so growth takes its room from the thread above and the box's
// bottom edge never moves.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => {},
  primaryRunIdFor: () => null,
}));
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: async () => {},
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notify: () => {} }));
const mountAgentTab = vi.fn(() => ({ dispose: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: (...args) => mountAgentTab(...args) }));

const { App } = await import("../src/app.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");

const message = (body) => ({ type: "message", data: { role: "agent", body, sequence: body.length } });

const branchRow = (items = []) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [{ id: "ag-1", ordinal: 1, provider: "claude", state: "live", unread_count: 0, working: false }],
  run: { run_id: "run-3", thread: { items, sessions: [] } },
});

let payload = branchRow();
let rail = null;

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
};

const panel = () => document.getElementById("rail-panel");
const scroller = () => document.getElementById("rail-body");
const composerRow = () => document.getElementById("rail-composer");

const mount = async () => {
  rail = mountAgentRail(document.getElementById("agent-rail"), {
    kind: "branch",
    projectId: "p1",
    branch: "build/login",
  });
  await flush();
};

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  mountAgentTab.mockClear();
  payload = branchRow();
  App.call = vi.fn(async (method) => (method === "branch.get" ? payload : {}));
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
  vi.useRealTimers();
});

describe("the conversation panel's column", () => {
  it("puts the composer beside the scroller, not inside it", async () => {
    await mount();
    expect(composerRow()).toBeTruthy();
    expect(scroller().contains(composerRow())).toBe(false);
    expect(composerRow().parentElement).toBe(panel());
    expect(scroller().parentElement).toBe(panel());
  });

  it("lays the panel out head, thread, composer — in that order", async () => {
    await mount();
    expect([...panel().children].map((child) => child.className)).toEqual([
      "rail-head",
      "rail-body",
      "rail-composer",
    ]);
  });

  it("leaves the scroller holding the conversation and nothing else", async () => {
    payload = branchRow([message("the agent replied")]);
    await mount();
    expect(scroller().querySelector(".thread-composer")).toBe(null);
    expect(scroller().querySelector("#railinput")).toBe(null);
    expect(scroller().textContent).toContain("the agent replied");
    expect(composerRow().querySelector("#railinput")).toBeTruthy();
  });

  it("keeps the pinned box through a poll that repaints the thread under it", async () => {
    await mount();
    const input = document.getElementById("railinput");
    input.focus();
    input.value = "half a thought";

    payload = branchRow([message("the agent replied")]);
    vi.advanceTimersByTime(2000);
    await flush();

    expect(document.getElementById("railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a thought");
    expect(scroller().textContent).toContain("the agent replied");
  });

  // The checkout was Build's by the time the next tick answered — an adopt a
  // sibling surface made while this box was being typed into. Only the words in
  // the placeholder changed, so only the words move.
  it("moves a changed placeholder onto the box rather than rebuilding it", async () => {
    payload = { ...branchRow(), run_id: null };
    await mount();
    const input = document.getElementById("railinput");
    expect(input.placeholder).toBe("Send a message to start an agent here…");

    payload = branchRow();
    vi.advanceTimersByTime(2000);
    await flush();

    expect(document.getElementById("railinput")).toBe(input);
    expect(input.placeholder).toBe("Send a message to this agent…");
  });

  it("gives the PTY the whole panel: no composer beside the agent's screen", async () => {
    await mount();
    panel().querySelector('[data-mode="tui"]').click();
    await flush();
    expect(composerRow()).toBe(null);
    expect(scroller().classList.contains("rail-body-tui")).toBe(true);

    panel().querySelector('[data-mode="chat"]').click();
    await flush();
    expect(composerRow()).toBeTruthy();
    expect(scroller().contains(composerRow())).toBe(false);
  });

  it("brings the draft back with the box when the panel returns from the terminal", async () => {
    await mount();
    document.getElementById("railinput").value = "unsent words";
    document.getElementById("railinput").dispatchEvent(new Event("input", { bubbles: true }));

    panel().querySelector('[data-mode="tui"]').click();
    await flush();
    panel().querySelector('[data-mode="chat"]').click();
    await flush();

    expect(document.getElementById("railinput").value).toBe("unsent words");
  });

  it("still sends what the pinned box holds", async () => {
    await mount();
    document.getElementById("railinput").value = "ship it";
    document.getElementById("railsend").click();
    await flush();

    const post = App.call.mock.calls.find(([method]) => method === "thread.post");
    expect(post[1]).toMatchObject({ entity_id: "run-3", agent_id: "ag-1", body: "ship it" });
  });
});

describe("the panel's stylesheet", () => {
  const panelRule = () => shellCss.match(/\.rail-panel \{[^}]*\}/)[0];
  const headRule = () => shellCss.match(/\.rail-head \{[^}]*\}/)[0];
  const bodyRule = () => shellCss.match(/\.rail-body \{[^}]*\}/)[0];
  const composerRule = () => shellCss.match(/\.rail-composer \{[^}]*\}/)[0];
  const composerTextRule = () => shellCss.match(/\.rail-composer \.composer textarea \{[^}]*\}/)[0];

  it("makes the panel a column whose middle row is the only scroller", () => {
    expect(panelRule()).toMatch(/display:flex/);
    expect(panelRule()).toMatch(/flex-direction:column/);
    expect(bodyRule()).toMatch(/flex:1 1 auto/);
    expect(bodyRule()).toMatch(/overflow-y:auto/);
    expect(bodyRule()).toMatch(/min-height:0/);
  });

  it("holds the composer out of the scroll, at the panel's bottom edge", () => {
    expect(headRule()).toMatch(/flex:none/);
    expect(composerRule()).toMatch(/flex:none/);
  });

  // Growth has to stop somewhere: a pasted paragraph that kept growing would
  // swallow the conversation it is a reply to.
  it("caps how far the box may grow upward", () => {
    expect(composerTextRule()).toMatch(/max-height:40vh/);
  });
});
