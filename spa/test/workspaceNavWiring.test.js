/** @vitest-environment jsdom */
// The workspace surface over a workspace with two directories, one of them not
// git (#174), with its panes mounted for real: the Files tree and the rail are
// the production modules, answering from one scripted machine. Only what is
// beside the surface — the console, the agent rail, and the feed delivering
// the machine's checkout list — is stood in for; workspaceViewDom holds the
// cases that need the panes stood in for.

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
  App.viewingContext = { set() {}, clear() {}, clearSelection() {} };
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
});
