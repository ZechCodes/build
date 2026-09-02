// @vitest-environment jsdom
// The conversation menu and what it opens.
//
// The header's ⋯ lists the surface kinds the agent actually has something in,
// and choosing one opens that surface as a modal over the panel: the same
// viewer the pills open, at whatever height it needs, fed every snapshot the
// rail reads for as long as it is up.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");
const appCss = readFileSync(resolve("src/styles.css"), "utf8");

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
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notify: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));

const { App } = await import("../src/app.js");
const { mountAgentRail, panelHeadHtml, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { openSurfaceOverlay } = await import("../src/core/agentSurfaces.js");
const { SHELL_ENTRY_KIND, WORKFLOW_ENTRY_KIND, rowActions, surfaceRows } = await import(
  "../src/core/agentSurfacesModel.js"
);

const surfaces = () => ({
  workflows: [
    {
      id: "wf-1",
      name: "Review sweep",
      state: "running",
      phases: [{ title: "Read", agents: [{ id: "a1", label: "reader", state: "running" }] }],
    },
  ],
  shells: [{ id: "sh1", description: "cargo test", state: "running", tail: ["running 12 tests"] }],
});

const branchRow = (agentOver = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [
    {
      id: "ag-1",
      ordinal: 1,
      provider: "claude",
      state: "live",
      unread_count: 0,
      working: false,
      surfaces: surfaces(),
      ...agentOver,
    },
  ],
  run: { run_id: "run-3", thread: { items: [], sessions: [] } },
});

let payload = branchRow();
let calls = [];
let rail = null;

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
};

const panel = () => document.getElementById("rail-panel");
const menuCaret = () => panel().querySelector(".rail-surface-menu .caret");
const menuItems = () => [...panel().querySelectorAll(".rail-surface-menu .mi")];
const menuItem = (kind) => panel().querySelector(`.rail-surface-menu .mi[data-action="${kind}"]`);
const openMenuElement = () => panel().querySelector(".rail-surface-menu .splitmenu");
const overlay = () => document.querySelector(".modal-surface");
const overlayRows = () => [...document.querySelectorAll(".modal-surface .surface-running > .surface-row")];
const pressEscape = () =>
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

const mount = async () => {
  rail = mountAgentRail(document.getElementById("agent-rail"), {
    kind: "branch",
    projectId: "p1",
    branch: "build/login",
  });
  await flush();
};

const poll = async (next) => {
  payload = next;
  vi.advanceTimersByTime(2000);
  await flush();
};

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  notifyError.mockClear();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  calls = [];
  payload = branchRow();
  App.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "branch.get") return payload;
    if (method === "thread.post") return { posted_sequence: 7 };
    return {};
  });
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("panelHeadHtml's surface menu", () => {
  it("writes no menu at all when the agent has nothing to show", () => {
    const html = panelHeadHtml("Claude Code", "chat", { surfaceOptions: [] });
    expect(html).toContain("rail-surface-menu");
    expect(html).not.toContain("splitbtn");
  });

  it("writes one item per kind, beside the remove and collapse buttons", () => {
    const html = panelHeadHtml("Claude Code", "chat", {
      removable: true,
      surfaceOptions: [{ id: SHELL_ENTRY_KIND, label: "Shells", description: "1 running" }],
    });
    expect(html).toContain(`data-action="${SHELL_ENTRY_KIND}"`);
    expect(html).toContain("Shells");
    expect(html).toContain("1 running");
    expect(html.indexOf("rail-surface-menu")).toBeLessThan(html.indexOf("rail-remove"));
    expect(html.indexOf("rail-remove")).toBeLessThan(html.indexOf("rail-collapse"));
  });
});

describe("openSurfaceOverlay", () => {
  it("mounts the kind's viewer in a modal and keeps it current", () => {
    const held = openSurfaceOverlay(SHELL_ENTRY_KIND, { onSendMessage: async () => {}, onOpenThreadItem: () => {} });
    held.set(surfaces());

    expect(overlay().querySelector("h3").textContent).toBe("Shells");
    expect(overlayRows()).toHaveLength(1);

    const grown = surfaces();
    grown.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });
    held.set(grown);
    expect(overlayRows()).toHaveLength(2);

    held.close();
    expect(overlay()).toBe(null);
  });

  it("keeps the empty viewer up when the kind loses everything under the reader", () => {
    const held = openSurfaceOverlay(SHELL_ENTRY_KIND, { onSendMessage: async () => {}, onOpenThreadItem: () => {} });
    held.set(surfaces());

    held.set({ workflows: surfaces().workflows });

    expect(overlay()).not.toBe(null);
    expect(overlay().querySelector(".surface-shells")).not.toBe(null);
    expect(overlayRows()).toEqual([]);
    held.close();
  });

  it("tells its caller once when Escape takes it away", () => {
    const onClose = vi.fn();
    const held = openSurfaceOverlay(WORKFLOW_ENTRY_KIND, {
      onSendMessage: async () => {},
      onOpenThreadItem: () => {},
      onClose,
    });
    held.set(surfaces());

    pressEscape();

    expect(overlay()).toBe(null);
    expect(onClose).toHaveBeenCalledTimes(1);
    held.close();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("the conversation header's menu", () => {
  it("offers one option per kind the agent has content in", async () => {
    await mount();
    expect(menuItems().map((item) => item.dataset.action)).toEqual([WORKFLOW_ENTRY_KIND, SHELL_ENTRY_KIND]);
    expect(menuItem(SHELL_ENTRY_KIND).textContent).toContain("Shells");
    expect(menuItem(SHELL_ENTRY_KIND).textContent).toContain("1 running");
  });

  it("is absent while the agent has no surfaces at all", async () => {
    payload = branchRow({ surfaces: null });
    await mount();
    expect(menuCaret()).toBe(null);
    expect(panel().querySelector(".rail-surface-menu")).not.toBe(null);
  });

  it("leaves an open menu open through a read that lists the same options", async () => {
    await mount();
    menuCaret().click();
    const menu = openMenuElement();

    const moved = branchRow();
    moved.agents[0].surfaces.shells[0].tail = ["running 12 tests", "test parser::folds ... ok"];
    await poll(moved);

    expect(openMenuElement()).toBe(menu);
    expect(menu.hidden).toBe(false);
  });

  it("repaints the options when the agent gains a kind", async () => {
    await mount();
    const gained = branchRow();
    gained.agents[0].surfaces.checklist = [{ id: "c1", subject: "Land the fold", state: "in_progress" }];

    await poll(gained);

    expect(menuItems().map((item) => item.dataset.action)).toEqual([
      WORKFLOW_ENTRY_KIND,
      SHELL_ENTRY_KIND,
      "checklist",
    ]);
  });

  it("goes away when the agent's last surface does", async () => {
    await mount();
    expect(menuCaret()).not.toBe(null);

    await poll(branchRow({ surfaces: {} }));

    expect(menuCaret()).toBe(null);
  });
});

describe("the surface a menu option opens", () => {
  const openShells = async () => {
    await mount();
    menuCaret().click();
    menuItem(SHELL_ENTRY_KIND).click();
  };

  it("shows the kind's rows over the panel", async () => {
    await openShells();
    expect(overlay()).not.toBe(null);
    expect(overlayRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(overlay().textContent).toContain("cargo test");
  });

  it("takes every later read while it is open", async () => {
    await openShells();
    const grown = branchRow();
    grown.agents[0].surfaces.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });

    await poll(grown);

    expect(overlayRows().map((row) => row.dataset.key)).toEqual(["sh1", "sh2"]);
  });

  it("posts a row's Ask straight to the agent", async () => {
    await openShells();
    const [row] = overlayRows();
    const [action] = rowActions(SHELL_ENTRY_KIND, surfaceRows(SHELL_ENTRY_KIND, surfaces())[0]);

    row.querySelector(".caret").click();
    row.querySelector(`.mi[data-action="${action.id}"]`).click();
    await flush();

    const posts = calls.filter((call) => call.method === "thread.post");
    expect(posts).toHaveLength(1);
    expect(posts[0].params.body).toBe(action.message);
  });

  it("closes on Escape and leaves nothing of itself behind", async () => {
    await openShells();
    pressEscape();
    await flush();

    expect(overlay()).toBe(null);
    expect(document.querySelector(".modal-scrim")).toBe(null);

    // The viewer went with it: a later read paints no rows anywhere off screen.
    await poll(branchRow());
    expect(document.querySelectorAll(".modal-surface")).toHaveLength(0);
  });

  it("closes when the panel it stands over changes agents", async () => {
    await openShells();
    const another = branchRow();
    another.agents.push({ ...another.agents[0], id: "ag-2", ordinal: 2, surfaces: null });
    await poll(another);
    [...document.querySelectorAll(".rail-bubble")][1].click();
    await flush();

    expect(overlay()).toBe(null);
  });
});

describe("the overlay's own height", () => {
  it("is a modal, not the rail's capped viewer region", async () => {
    await mount();
    menuCaret().click();
    menuItem(SHELL_ENTRY_KIND).click();

    expect(overlay().closest(".rail-surfaces-viewer")).toBe(null);
    expect(shellCss).toMatch(/\.rail-surfaces-viewer:not\(:empty\)\s*\{[^}]*max-height:34vh/);
    expect(shellCss).not.toContain(".modal-surface");
    expect(appCss).toMatch(/\.modal\.modal-surface\s*\{[^}]*max-height/);
    expect(appCss).toMatch(/\.modal-surface\s+\.surface-overlay-body\s*\{[^}]*overflow-y:auto/);
  });
});
