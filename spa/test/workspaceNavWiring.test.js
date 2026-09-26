/** @vitest-environment jsdom */
// The workspace surface over a workspace with two directories, one of them not
// git (#174), with its panes mounted for real: the Files tree, the Changes tab
// row over the git pane, and the rail are the production modules, answering
// from one scripted machine. Only what is beside the surface — the console,
// the agent rail, and the feed delivering the machine's checkout list — is
// stood in for; workspaceViewDom holds the cases that need the panes stood in
// for.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

vi.mock("../src/core/console.js", () => ({ mountConsole: vi.fn(() => ({ dispose: vi.fn() })) }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: vi.fn(() => ({ dispose: vi.fn() })) }));

let feedWorkspaces = [];
let feedSubscribers = new Set();
const feedSnapshot = () => ({ devices: { "dev-1": { items: [], projects: [], workspaces: feedWorkspaces } } });
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    feedSubscribers.add(fn);
    fn(feedSnapshot());
    return () => feedSubscribers.delete(fn);
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [true],
  deliverFeed: () => {},
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));

import { App } from "../src/app.js";
import { renderWorkspace } from "../src/views/workspaceView.js";
import { adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { standShell, stopShell } from "../src/core/shell.js";
import { fakeSession } from "./deviceSessionFixture.js";
import { wipeCache } from "../src/core/localCache.js";
import { createViewingContext } from "../src/core/viewingContext.js";

const b64 = (text) => Buffer.from(text, "utf8").toString("base64");

const workspace = {
  id: "ws-1",
  project_id: "p-1",
  name: "payment-work",
  directories: [
    { source_id: "repo", name: "Repository", is_git: true },
    { source_id: "assets", name: "Assets", is_git: false },
  ],
};

const TREES = {
  repo: { "": [{ name: "README.md", kind: "file", size: 6 }] },
  assets: { "": [{ name: "logo.svg", kind: "file", size: 6 }] },
};

const asked = [];
const machine = async (method, params = {}) => {
  asked.push({ method, params });
  if (method === "fs.tree") return { path: params.path, entries: TREES[params.source_id]?.[params.path] || [] };
  if (method === "fs.read") {
    const text = `${params.source_id}/${params.path}`;
    return { path: params.path, mime: "text/plain", size: text.length, truncated: false, editable: false, encoding: "utf-8", revision: "r1", content_b64: b64(text) };
  }
  if (method === "git.status") return { branch: "main", head: "c0ffee1", repo_state: "clean", files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } };
  if (method === "git.log") return { branch: "main", commits: [{ hash: "c0ffee1234567", short: "c0ffee1", subject: "Seed the repository", author: "Zech", time: 1_790_000_000 }], more: false };
  if (method === "git.refs") return { current: { kind: "branch", name: "main", full_ref: "refs/heads/main" }, refs: [] };
  if (method === "git.unpushed") return { commits: [], files: [] };
  if (method === "workspace.git_init_options") return {
    workspace_id: "ws-1", source_id: params.source_id,
    workspace: { path: "/w/ws-1/assets", is_git: false, available: true },
    source: { path: "/src/assets", is_git: false, available: true },
  };
  if (method === "workspace.init_git") return {
    workspace: { ...workspace, directories: [workspace.directories[0], { ...workspace.directories[1], is_git: true }] },
    outcomes: [{ target: "workspace", status: "initialized", is_git: true }],
    source: { source_id: "assets", path: "/src/assets", is_git: false },
  };
  return {};
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const rowIn = (root, path) =>
  [...document.querySelectorAll(".froot")].find((one) => one.dataset.root === root)?.querySelector(`.frow[data-path="${path}"]`);

const open = async (route) => {
  App.route = route;
  standShell(App.route);
  await renderWorkspace();
  await flush();
};

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = '<div id="toolbar"><span id="tb-verb"></span></div><nav id="dir-rail"></nav><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  App.viewDispose = null;
  App.viewingContext = createViewingContext({ enabled: false });
  App.devices = [{ id: "dev-1", name: "this machine", status: "online" }];
  asked.length = 0;
  feedSubscribers = new Set();
  feedWorkspaces = [workspace];
  adoptDeviceSession({ ...fakeSession("dev-1"), call: vi.fn(machine) });
  window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
});

afterEach(() => {
  App.viewDispose?.();
  App.viewDispose = null;
  stopShell();
  resetDeviceContexts();
  delete window.matchMedia;
});

describe("a workspace with two directories, one not git", () => {
  it("draws Files as one tree with a root per directory, and opens a file from the second", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", tab: "files" });
    await vi.waitFor(() => {
      expect(rowIn("repo", "README.md")).toBeTruthy();
      expect(rowIn("assets", "logo.svg")).toBeTruthy();
    });
    expect([...document.querySelectorAll("[data-root-head]")].map((head) => head.textContent.trim())).toEqual(["▸Repository", "▸Assets"]);
    expect(App.route.sourceId).toBe("repo");

    rowIn("assets", "logo.svg").click();
    await vi.waitFor(() => expect(document.querySelector(".fppath")?.textContent).toBe("logo.svg"));
    expect(App.route).toMatchObject({ sourceId: "assets", file: "logo.svg" });
    expect(asked).toContainEqual({ method: "fs.read", params: { workspace_id: "ws-1", source_id: "assets", path: "logo.svg" } });
  });

  it("stands the rail as the workspace's navigation beside it", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", tab: "files" });
    expect([...document.querySelectorAll("#dir-rail [data-tab]")].map((tab) => tab.dataset.tab)).toEqual(["changes", "files", "issues"]);
    expect(document.querySelector("#dir-rail [data-rail-settings]")).not.toBeNull();
  });

  it("names both directories over Changes, and offers Git as the surface of the one without it", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "changes" });
    expect([...document.querySelectorAll(".workspace-dirtab")].map((tab) => [tab.textContent, tab.getAttribute("aria-selected")])).toEqual([
      ["Repository", "false"],
      ["Assets", "true"],
    ]);
    expect(document.querySelector(".workspace-gitinit [data-init-git]").textContent).toBe("Initialize Git…");
    expect(document.querySelector(".crail-host")).toBeNull();
  });

  it("stands the git pane of the directory with git under the same row", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" });
    const row = document.querySelector(".workspace-dirtabs");
    expect(row.querySelector("[aria-selected=true]").textContent).toBe("Repository");
    await vi.waitFor(() => expect(row.nextElementSibling.querySelector(".gitpane .crail-host")).not.toBeNull());
    await vi.waitFor(() => expect(asked.some(({ method, params }) => method.startsWith("git.") && params.source_id === "repo")).toBe(true));
    expect(document.querySelector(".workspace-gitinit")).toBeNull();
  });

  it("initializes the directory without git, and its surface becomes the commit rail", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "changes" });
    const row = document.querySelector(".workspace-dirtabs");
    document.querySelector(".workspace-gitinit [data-init-git]").click();
    await vi.waitFor(() => expect(document.querySelector('[data-init-target="workspace"]')).not.toBeNull());
    document.querySelector('[data-init-target="workspace"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    await vi.waitFor(() => expect(document.querySelector(".gitpane .crail-host")).not.toBeNull());
    expect(document.querySelector(".workspace-gitinit")).toBeNull();
    expect(asked).toContainEqual({ method: "workspace.init_git", params: { workspace_id: "ws-1", source_id: "assets", target: "workspace" } });
    // The same row, still on Assets: the surface changed under it, not the view.
    expect(document.querySelector(".workspace-dirtabs")).toBe(row);
    expect(row.querySelector("[aria-selected=true]").textContent).toBe("Assets");
    // The original source still has no git, so its offer hangs off the rail.
    await vi.waitFor(() => expect(document.querySelector(".crail-host [data-init-git]")?.textContent).toBe("Initialize original source…"));
  });

  // Selecting a tab moves within Changes: a directory already shown is shown
  // again over the records it holds, and nothing is asked of the machine.
  it("goes back to a directory it has shown without asking the machine for anything", async () => {
    await open({ name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" });
    const reads = (sourceId) => asked.filter(({ method, params }) => params.source_id === sourceId && method !== "fs.tree");
    await vi.waitFor(() => expect(document.querySelector('[data-surface="repo"]')?.textContent).toContain("Seed the repository"));
    await vi.waitFor(() => expect(reads("repo").map(({ method }) => method)).toEqual(expect.arrayContaining(["git.status", "git.log", "git.refs", "git.unpushed"])));
    // Settled: the first visit's reads have all answered.
    let seen = -1;
    await vi.waitFor(async () => {
      const now = asked.length;
      await new Promise((resolve) => setTimeout(resolve, 30));
      expect(asked.length === now && now === (seen = now)).toBe(true);
    });
    const repoSurface = document.querySelector('[data-surface="repo"]');
    asked.length = 0;

    document.querySelector('.workspace-dirtab[data-directory="assets"]').click();
    expect(App.route).toMatchObject({ sourceId: "assets", tab: "changes" });
    await vi.waitFor(() => expect(document.querySelector('[data-surface="assets"] .workspace-gitinit [data-init-git]')).not.toBeNull());
    expect(repoSurface.hidden).toBe(true);

    document.querySelector('.workspace-dirtab[data-directory="repo"]').click();
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(App.route).toMatchObject({ sourceId: "repo", tab: "changes" });
    // The very surface it left, showing, with what it held.
    expect(document.querySelector('[data-surface="repo"]')).toBe(repoSurface);
    expect(repoSurface.hidden).toBe(false);
    expect(document.querySelector('[data-surface="assets"]').hidden).toBe(true);
    expect(repoSurface.textContent).toContain("Seed the repository");
    expect(document.querySelector(".workspace-dirtab.current").textContent).toBe("Repository");
    expect(reads("repo")).toEqual([]);
    expect(asked.filter(({ method }) => method.startsWith("git."))).toEqual([]);
  });
});
