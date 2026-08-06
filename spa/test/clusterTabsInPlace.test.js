// @vitest-environment jsdom
// A project-level tab is a tab, not a destination.
//
// Inbox, Issues and Archive are project-scoped, so they ride every project
// surface's right cluster — and selecting one must paint into the surface the
// user is standing on. Switching to a project tab from a worktree or a run may
// never navigate to the primary checkout: the route stays that surface's, so
// picking a surface tab afterwards lands back on the worktree/run, and a reload
// returns to it too.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// The surfaces mount terminal/agent panes over the shared terminal socket; here
// the manager is a double, so no surface reaches a real WebSocket.
const fakeManager = {
  listTerminals: vi.fn(async () => []),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  attachAgent: vi.fn(async () => ({ term_id: "agent:worktree", live: true, snapshot: "", cursor: 0 })),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => fakeManager,
  subscribeTerminalStatus: () => () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

import { App } from "../src/app.js";
import { renderWorktree } from "../src/views/worktree.js";
import { renderTask } from "../src/views/task.js";
import { renderMain } from "../src/views/mainWorktree.js";

const runPayload = () => ({
  run_id: "r-1",
  state: "building",
  goal: "a goal",
  project_id: "p-1",
  branch: "build/thing",
  worktree_path: "/tmp/wt",
  stages: [],
  thread: { items: [] },
});

const rpc = (method) => {
  if (method === "run.get") return runPayload();
  if (method === "board.list") return { runs: [], plans: [], external_worktrees: [] };
  if (method === "archive.list") return { plans: [], worktrees: [] };
  if (method === "git.status") return { branch: "build/thing", path: "/tmp/wt", files: [], commits: [] };
  return {};
};

const clickTab = (tabId) => document.querySelector(`.tabs .t[data-tab="${tabId}"]`).click();
const openMenuAction = (action) => {
  document.querySelector(".tabs .tmenu").click();
  document.querySelector(`.tabmenu [data-action="${action}"]`).click();
};
const body = () => document.querySelector("#tabbody");

describe("project-level tabs stay on the surface that selected them", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="root"></div><div id="sheet"></div><div id="scrim"></div>';
    App.offline = false;
    App.poll = null;
    App.viewDispose = null;
    App.call = vi.fn(async (method) => rpc(method));
  });
  afterEach(() => {
    if (App.viewDispose) App.viewDispose();
    if (App.poll) clearInterval(App.poll);
    App.poll = null;
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("worktree: Issues paints in the worktree's body and leaves the route on the worktree", async () => {
    App.route = { name: "worktree", projectId: "p-1", worktreeId: "w-1", tab: "conversation" };
    location.hash = "#/project/p-1/worktree/w-1/conversation";
    await renderWorktree();
    await vi.advanceTimersByTimeAsync(0);

    clickTab("issues");
    await vi.advanceTimersByTimeAsync(0);
    expect(body().querySelector(".issues")).toBeTruthy();
    expect(App.route).toEqual({ name: "worktree", projectId: "p-1", worktreeId: "w-1", tab: "issues" });
    expect(location.hash).toBe("#/project/p-1/worktree/w-1/issues");
    expect(document.querySelector('.tabs .ticon[data-tab="issues"]').classList.contains("active")).toBe(true);

    // …and a surface tab afterwards returns to the worktree the user was on.
    clickTab("changes");
    await vi.advanceTimersByTimeAsync(0);
    expect(App.route).toEqual({ name: "worktree", projectId: "p-1", worktreeId: "w-1", tab: "changes" });
    expect(location.hash).toBe("#/project/p-1/worktree/w-1/changes");
    expect(body().querySelector(".issues")).toBeNull();
  });

  it("worktree: Inbox and Archive stay in place too", async () => {
    App.route = { name: "worktree", projectId: "p-1", worktreeId: "w-1", tab: "conversation" };
    location.hash = "#/project/p-1/worktree/w-1/conversation";
    await renderWorktree();
    await vi.advanceTimersByTimeAsync(0);

    clickTab("inbox");
    await vi.advanceTimersByTimeAsync(0);
    expect(body().querySelector(".project-inbox")).toBeTruthy();
    expect(location.hash).toBe("#/project/p-1/worktree/w-1/inbox");

    openMenuAction("archive");
    await vi.advanceTimersByTimeAsync(0);
    expect(App.route.name).toBe("worktree");
    expect(location.hash).toBe("#/project/p-1/worktree/w-1/archive");
  });

  it("run: Issues paints in the run's body and leaves the route on the run", async () => {
    App.route = { name: "task", projectId: "p-1", id: "r-1", tab: "conversation" };
    location.hash = "#/project/p-1/task/r-1/conversation";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);

    clickTab("issues");
    await vi.advanceTimersByTimeAsync(0);
    expect(body().querySelector(".issues")).toBeTruthy();
    expect(App.route).toEqual({ name: "task", projectId: "p-1", id: "r-1", tab: "issues" });
    expect(location.hash).toBe("#/project/p-1/task/r-1/issues");

    // The 1.6s poll leaves the mounted pane alone: the run keeps ticking under a
    // project tab without repainting over it or moving the route.
    await vi.advanceTimersByTimeAsync(1600);
    expect(body().querySelector(".issues")).toBeTruthy();
    expect(location.hash).toBe("#/project/p-1/task/r-1/issues");

    clickTab("conversation");
    await vi.advanceTimersByTimeAsync(0);
    expect(App.route).toEqual({ name: "task", projectId: "p-1", id: "r-1", tab: "conversation" });
    expect(location.hash).toBe("#/project/p-1/task/r-1/conversation");
  });

  // A run entered by URL learns its project from its first run.get; the cluster
  // (Issues included) appears with it rather than mounting against no project.
  it("run: the cluster arrives with the project the run reports", async () => {
    App.route = { name: "task", id: "r-1", tab: "conversation" };
    location.hash = "#/task/r-1/conversation";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);

    expect(document.querySelector('.tabs .ticon[data-tab="issues"]')).toBeTruthy();
    clickTab("issues");
    await vi.advanceTimersByTimeAsync(0);
    expect(body().querySelector(".issues")).toBeTruthy();
    expect(App.route).toEqual({ name: "task", projectId: "p-1", id: "r-1", tab: "issues" });
  });
});

// The tab bar is a tab bar. The branch is in the Changes pane's own header, in
// the sidebar, and in every commit the surface shows — repeating it in the row
// bought nothing and cost the row's right edge.
describe("no surface repeats its branch in the tab bar", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="root" class="surface"></div><div id="sheet"></div><div id="scrim"></div>';
    App.offline = false;
    App.poll = null;
    App.viewDispose = null;
    App.call = vi.fn(async (method) => rpc(method));
  });
  afterEach(() => {
    if (App.viewDispose) App.viewDispose();
    if (App.poll) clearInterval(App.poll);
    App.poll = null;
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  const barIsBare = () => {
    expect(document.querySelector(".surface-meta")).toBeNull();
    expect(document.querySelector(".surface-bar").textContent).not.toContain("build/thing");
  };

  it("primary checkout", async () => {
    App.route = { name: "project", projectId: "p-1", tab: "conversation" };
    await renderMain();
    await vi.advanceTimersByTimeAsync(0);
    barIsBare();
  });

  it("external worktree", async () => {
    App.route = { name: "worktree", projectId: "p-1", worktreeId: "w-1", tab: "changes" };
    location.hash = "#/project/p-1/worktree/w-1/changes";
    await renderWorktree();
    await vi.advanceTimersByTimeAsync(0);
    barIsBare();
  });

  it("run", async () => {
    App.route = { name: "task", projectId: "p-1", id: "r-1", tab: "conversation" };
    location.hash = "#/project/p-1/task/r-1/conversation";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);
    barIsBare();
  });
});
