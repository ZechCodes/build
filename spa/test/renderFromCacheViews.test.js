/** @vitest-environment jsdom */
// Every route surface paints what the cache holds, whatever the connection is
// doing (ARCHITECTURE.md, Render from cache): "If the device is connecting,
// everything should render from local cache as if it is connected."
//
// Each surface is mounted over records a past session left on disk while its
// machine is in each of the three states the connection model can report for a
// machine it is not talking to: dialling it on a cold reload before anything
// has answered in this tab, dialling it again after a session it had was lost,
// and holding no session with nothing being done about it. In every one of them
// the cached content is on screen, and no "Connecting to …" notice stands where
// it should be.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { issue as trackerIssue } from "./trackerWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { recovery, renderFilesTab, mountAgentRail } = vi.hoisted(() => ({
  recovery: new Map(), // deviceId → the recovery record the supervisor would hold
  mountAgentRail: vi.fn(() => ({ dispose() {} })),
  renderFilesTab: vi.fn((host) => {
    host.innerHTML = '<main class="file-editor"></main>';
    return { dispose: vi.fn(), canLeave: vi.fn(async () => true) };
  }),
}));

// The recovery supervisor is what says a machine is being dialled. It is
// scripted here rather than run, so no case opens a socket.
vi.mock("../src/core/deviceRecovery.js", () => ({
  createDeviceRecoverySupervisor: () => ({
    snapshot: (deviceId) => recovery.get(deviceId) || null,
    subscribe: () => () => {},
    syncPresence() {},
    wake: () => false,
    reset() {},
    beginAttempt() {},
    connected() {},
    failed() {},
    recoverNow() {},
    stop() {},
  }),
}));
// The rail and the console are the shell's, not these surfaces': they are
// counted, not run.
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail }));
vi.mock("../src/core/console.js", () => ({ mountConsole: () => ({ dispose() {} }) }));
// The Files tab reads the directory on demand (a documented exception); the
// workspace surface's own content is the checkout it mounts that tab over.
vi.mock("../src/views/files.js", () => ({ renderFilesTab }));

import { App } from "../src/app.js";
import { adoptDeviceSession, resetDeviceContexts, setContextOffline } from "../src/core/deviceContexts.js";
import { DEVICES_ADDRESS, wipeCache, writeCached } from "../src/core/localCache.js";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { entityIdOf } from "../src/core/entityId.js";
import { startFeed, stopFeed } from "../src/core/taskFeed.js";
import { standShell, stopShell } from "../src/core/shell.js";
import { issueAddress } from "../src/core/issueCache.js";
import { writeIssueRecord } from "../src/core/trackerCache.js";
import { fakeSession } from "./deviceSessionFixture.js";
import { renderBranch } from "../src/views/branchView.js";
import { renderIssue } from "../src/views/issueView.js";
import { renderTrackerIssue } from "../src/views/trackerIssueView.js";
import { renderProject } from "../src/views/projectView.js";
import { renderWorkspace } from "../src/views/workspaceView.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const settle = async () => {
  for (let turn = 0; turn < 30; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

const DEVICE = "dev-1";
const devicesListing = (status) => [{ id: DEVICE, name: "Desktop", status }];

/** The machine's session, which answers whatever it is asked with nothing. */
let bridge = null;
const adopt = () => {
  bridge = fakeSession(DEVICE);
  adoptDeviceSession(bridge);
};

/**
 * The three states the connection model reports for a machine that cannot
 * answer. None of them is "connected"; each case paints as if it were.
 */
const CONNECTION_STATES = {
  // A cold reload: the account lists the machine, nothing has answered in this
  // tab yet, and the supervisor is dialling it.
  connecting: () => {
    App.devices = devicesListing("online");
    recovery.set(DEVICE, { deviceId: DEVICE, status: "attempting", failedAttempts: 0, nextAttemptAt: null });
  },
  // A session it had was lost, and the supervisor is between tries.
  reconnecting: () => {
    App.devices = devicesListing("online");
    adopt();
    setContextOffline(DEVICE);
    recovery.set(DEVICE, { deviceId: DEVICE, status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 4000 });
  },
  // The account calls it offline: no session, and nothing is dialling it.
  disconnected: () => {
    App.devices = devicesListing("offline");
    adopt();
    setContextOffline(DEVICE);
    recovery.delete(DEVICE);
  },
};

const workspace = {
  id: "ws-1",
  name: "wire-facade",
  project_id: "p1",
  directories: [{ source_id: "assets", name: "Assets", is_git: false }],
};
const branchRow = { kind: "branch", project_id: "p1", project: "notes", branch: "build/login", worktree_id: "wt-1", agents: [] };

/** The board as the last session's pass left it on disk, read by the feed. */
async function cacheBoard() {
  const view = liveFeedSnapshot(
    { items: [branchRow], runs: [] },
    { projects: [{ project_id: "p1", name: "notes", is_git: true, entity_id: "run-project" }] },
    { workspaces: [workspace] },
    DEVICE,
  );
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "feed" }, view);
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "projects" }, view.projects);
  await writeCached({ deviceId: DEVICE, entityId: "", kind: "workspaces" }, view.workspaces);
  for (const item of view.items) {
    const entityId = entityIdOf(item);
    if (entityId) await writeCached({ deviceId: DEVICE, entityId, kind: "row" }, item);
  }
}

/** Each surface: the route it stands on, the renderer, what else the last
 *  session left for it, and what of that cache must be on screen. */
const SURFACES = {
  branch: {
    route: { name: "branch", deviceId: DEVICE, projectId: "p1", branch: "build/login", tab: "changes" },
    render: renderBranch,
    painted: () => document.querySelector("#tabbody .gitpane"),
  },
  issue: {
    route: { name: "issue", deviceId: DEVICE, projectId: "p1", id: "issue-1" },
    render: renderIssue,
    seed: async () => {
      await writeCached(issueAddress(DEVICE, "issue-1", "get"), {
        issue_id: "issue-1", plan_id: "issue-1", project_id: "p1", project: "notes",
        goal: "Rebuild the issue view", state: "plan_review", base_branch: "main",
        stages: [{ id: "s1", state: "planned" }], implementation_lineage: [],
        thread: { items: [], thread_last_sequence: 1 },
      });
      await writeCached(issueAddress(DEVICE, "issue-1", "stages"), {
        stages: [{ id: "s1", title: "Cached stage", state: "planned", approval: "planned", execution: "pending", open_comments: 0, comments: [] }],
      });
    },
    painted: () => document.querySelector("#tabbody .ivstages")?.textContent.includes("Cached stage"),
  },
  trackerIssue: {
    route: { name: "trackerIssue", deviceId: DEVICE, projectId: "p1", issueId: "issue-1" },
    render: renderTrackerIssue,
    seed: () => writeIssueRecord(DEVICE, "p1", "issue-1", {
      issue: trackerIssue({ id: "issue-1", number: 12, title: "The cached issue title" }),
      timeline: [],
    }),
    painted: () => document.querySelector("#issue-pane")?.textContent.includes("The cached issue title"),
  },
  project: {
    route: { name: "project", deviceId: DEVICE, projectId: "p1", tab: "workspaces" },
    render: renderProject,
    painted: () => document.querySelector(`#project-pane [data-workspace="${DEVICE}/ws-1"]`)?.textContent.includes("wire-facade"),
  },
  workspace: {
    route: { name: "workspace", deviceId: DEVICE, projectId: "p1", workspaceId: "ws-1", sourceId: "assets", tab: "files" },
    render: renderWorkspace,
    painted: () => renderFilesTab.mock.calls.length > 0 && document.querySelector("#tabbody .file-editor"),
  },
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  document.getElementById("toolbar").innerHTML = '<span id="tb-verb"></span>';
  await wipeCache();
  renderFilesTab.mockClear();
  mountAgentRail.mockClear();
  recovery.clear();
  bridge = null;
  App.viewDispose = null;
  App.poll = null;
  App.viewingContext = { set() {}, clear() {} };
  App.selectedDeviceId = DEVICE;
  App.devices = devicesListing("online");
  await writeCached(DEVICES_ADDRESS, App.devices);
  await cacheBoard();
  await startFeed();
  await settle();
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  App.viewDispose?.();
  App.viewDispose = null;
  stopShell();
  stopFeed();
  resetDeviceContexts();
});

describe.each(Object.entries(SURFACES))("the %s surface paints from the cache", (_name, surface) => {
  it.each(Object.keys(CONNECTION_STATES))("while its machine is %s", async (state) => {
    await surface.seed?.();
    CONNECTION_STATES[state]();
    App.route = { ...surface.route };

    standShell(App.route);
    await surface.render();
    await settle();

    expect(document.getElementById("root").textContent).not.toContain("Connecting to");
    expect(surface.painted()).toBeTruthy();
    // A machine being dialled is painted as if it answered: nothing over the
    // surface says otherwise. Only one nothing is dialling is named.
    expect(Boolean(document.querySelector("#root > .device-strip"))).toBe(state === "disconnected");
    // The rail beside it pages its conversation out of the same records, so it
    // stands too.
    expect(mountAgentRail).toHaveBeenCalledOnce();
    // Nothing a surface painted came off the wire: no session was answering.
    if (bridge) expect(bridge.call).not.toHaveBeenCalled();
  });
});
