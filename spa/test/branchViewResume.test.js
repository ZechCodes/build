// @vitest-environment jsdom
// A surface whose machine drops and comes back must ask the machine, not the
// socket it was mounted over.
//
// A device that goes and resumes keeps its context — its drafts, its cached
// reads and the surface standing on it all survive — and only its transport is
// replaced. A view that captured the old session's `call` at mount would keep
// asking a session that is closed: every RPC it makes is refused for want of a
// carrier, on a surface that claims to be live, until the reader navigates.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

import { fakeSession } from "./deviceSessionFixture.js";

// The surface writes what it reads through the local cache, which is keyed by
// device: give the module a fake IndexedDB to do it in.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const row = {
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  // The project's own checkout: no run, no worktree, so it is no entity the
  // board names and the sync layer never walks it. This surface is the only
  // reader of it, which is what makes the question below askable at all.
  worktree_id: null,
  agents: [],
  state: "review",
  primary: false,
  issue_id: null,
  stat: { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
};

/** One bridge, answering everything this surface asks of it. */
const bridge = () =>
  vi.fn(async (method) => {
    if (method === "board.list") return { items: [row] };
    if (method === "project.list") return { projects: [{ project_id: "p1", name: "relaydb", is_git: true, base_branch: "main" }] };
    if (method === "branch.get") return row;
    if (method === "git.status") return { files: [], head: "abc", status_key: "clean" };
    if (method === "git.log") return { commits: [] };
    return {};
  });

const askedFor = (call, method) => call.mock.calls.some(([asked]) => asked === method);

let App;
let renderBranch;
let adoptDeviceSession;
let resetDeviceContexts;
let refetchEverything;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/device/dev-1/project/p1/branch/build%2Flogin/changes";
  ({ App } = await import("../src/app.js"));
  ({ renderBranch } = await import("../src/views/branchView.js"));
  ({ adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js"));
  ({ refetchEverything } = await import("../src/core/changeEvents.js"));
  App.devices = [{ id: "dev-1", name: "This device", status: "online" }];
  App.selectedDeviceId = "dev-1";
  App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
  document.getElementById("toolbar").innerHTML = '<span id="tb-verb"></span>';
});

afterEach(() => {
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
  App.poll = null;
  resetDeviceContexts();
});

describe("a mounted branch surface across a resume", () => {
  it("reads through the session its device is on now, not the one it mounted over", async () => {
    const dropped = bridge();
    adoptDeviceSession({ ...fakeSession("dev-1"), call: dropped });
    await renderBranch();
    await flush();
    expect(askedFor(dropped, "git.status")).toBe(true);

    // The device went and came back: same context, new transport.
    const resumed = bridge();
    adoptDeviceSession({ ...fakeSession("dev-1"), call: resumed });
    dropped.mockClear();
    refetchEverything();
    await flush();

    expect(askedFor(resumed, "git.status")).toBe(true);
    expect(askedFor(dropped, "git.status")).toBe(false);
  });
});
