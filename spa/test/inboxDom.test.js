// @vitest-environment jsdom
// The rail, painted from every device's workspaces at once.
//
// A workspace belongs to one machine, so every row carries the machine that
// answered for it and the account-wide names minted from it
// (core/deviceKey.js): a row's verbs go to its own device, a row whose machine
// is away is greyed with every verb it had still on it, and the picker narrows
// the list without touching the route.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceOfflineWord } from "../src/core/text.js";
import { task } from "./trackerWireFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const navigate = vi.fn();
const createWorkspace = vi.fn();
// Every reader of the feed, not just the rail: the compose box reads it too.
let subscribers = [];
let snapshot = { items: [], pending: [], projects: [], workspaces: [], devices: {} };
const deliver = () => subscribers.forEach((fn) => fn(snapshot));
// The feed catching up is its own event: a verb that asks for a refresh does
// not deliver a snapshot here, so a row leaving is the wiring's own doing.
const refreshFeed = vi.fn(async () => []);

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscribers.push(fn);
    fn(snapshot);
    return () => {
      subscribers = subscribers.filter((each) => each !== fn);
    };
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  deliverFeed: () => deliver(),
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));
vi.mock("../src/core/inboxShell.js", () => ({ goFromInbox: (...args) => navigate(...args) }));
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => createWorkspace(...args) }));
const projectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: (...args) => projectSettings(...args) }));
const newRepoSheet = vi.fn();
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: (...args) => newRepoSheet(...args) }));

// What hiding a project does to the cache is core/projectHide.js's own business
// (test/projectHide.test.js). What matters here is the half the rail can see:
// the project's rows leave the snapshot the feed is serving, and the rail
// repaints without them. So the mock prunes this file's snapshot exactly as the
// real drop prunes the feed's, and delivers.
let hidden = [];
vi.mock("../src/core/projectHide.js", () => ({
  hideProject: async (target) => {
    hidden.push(target);
    for (const field of ["items", "projects", "workspaces"]) {
      snapshot[field] = (snapshot[field] || []).filter((row) => row.projectKey !== target.projectKey);
    }
    deliver();
  },
}));

const key = (deviceId, id) => `${deviceId}/${id}`;

/** A project as the feed stamps it. */
const project = (id, name, deviceId = "dev-1", over = {}) => ({
  id,
  project_id: id,
  name,
  deviceId,
  projectKey: key(deviceId, id),
  ...over,
});

/** A workspace as the feed stamps it: the machine it is on, and the two
 *  account-wide names minted from it. */
const workspace = (overrides = {}) => {
  const base = {
    id: "workspace-1",
    project_id: "project-1",
    name: "Checkout",
    root: "/work/checkout",
    status: "ready",
    deviceId: "dev-1",
    work_summary: { pushes: 2, behind: 1, additions: 8, deletions: 3 },
    directories: [{ id: "api", source_id: "source-api", is_git: true }],
    ...overrides,
  };
  return {
    ...base,
    projectKey: key(base.deviceId, base.project_id),
    workspaceKey: key(base.deviceId, base.id),
  };
};

/** A capture the router has not placed yet: work the account is holding, in no
 *  project and so under no block. */
const capture = (over = {}) => ({
  kind: "capture",
  capture_id: "cap-1",
  deviceId: "dev-1",
  project_id: "",
  projectKey: "",
  title: "fix the redirect",
  text: "fix the redirect",
  state: "routing",
  created_at: new Date(Date.now() - 60_000).toISOString(),
  ...over,
});

// `devices` is each machine's own slice of the same snapshot, which is what a
// surface about one machine reads (core/deviceContexts.js `deviceFeedView`):
// the reroute picker offers the projects of the machine holding the capture.
const feed = (
  workspaces,
  projects = [project("project-1", "Payments"), project("project-2", "Website")],
  items = [],
  devices = { "dev-1": { items, projects, workspaces } },
) => {
  snapshot = { items, pending: [], projects, workspaces, devices };
  deliver();
};

// Every project also has its project agent's row (#103, inboxRows103.test.js);
// these tests are about the workspace and capture rows.
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry:not(.inbox-project-agent)")];
const blocks = () => [...document.querySelectorAll("#inbox-list .inbox-project")];

let App;
let mountInboxList;
let unmountInboxList;
let inboxListRouteChanged;
let openNewProject;
let setInboxView;
let adoptBridgeSelection;
let adoptDeviceSession;
let contextFor;
let resetDeviceContexts;
let setContextOffline;
let rememberDeviceFilter;

/** A paired device, answering with `call`. */
const deviceAnswering = (deviceId, call) =>
  adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });

let workshopCall;
let laptopCall;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  subscribers = [];
  hidden = [];
  navigate.mockReset();
  createWorkspace.mockReset();
  newRepoSheet.mockReset();
  projectSettings.mockReset();
  refreshFeed.mockClear();
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  ({ App } = await import("../src/app.js"));
  ({ mountInboxList, unmountInboxList, inboxListRouteChanged, openNewProject, setInboxView } = await import("../src/core/inboxView.js"));
  ({ adoptBridgeSelection, adoptDeviceSession, contextFor, resetDeviceContexts, setContextOffline } = await import(
    "../src/core/deviceContexts.js"
  ));
  ({ rememberDeviceFilter } = await import("../src/core/deviceFilter.js"));
  resetDeviceContexts();
  App.route = { name: "inbox" };
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  App.selectedDeviceId = "dev-1";
  App.deviceFilter = null;
  workshopCall = vi.fn(async () => ({}));
  laptopCall = vi.fn(async () => ({}));
  deviceAnswering("dev-1", workshopCall);
  deviceAnswering("dev-2", laptopCall);
  snapshot = { items: [], pending: [], projects: [], workspaces: [], devices: {} };
  mountInboxList();
});

afterEach(() => unmountInboxList?.());

describe("the workspace inbox", () => {
  it("moves a workspace into the flat face's Recent after a day without a message", async () => {
    const old = Date.now() - 25 * 60 * 60 * 1000;
    feed([workspace({ session_started_ms: old, last_activity_ms: old })]);
    expect(document.querySelectorAll('#inbox-list > [data-key^="workspace:"]')).toHaveLength(0);
    expect(document.querySelector('[data-recent-toggle="inbox"]')).not.toBeNull();
    document.querySelector('[data-recent-toggle="inbox"]').click();
    await vi.waitFor(() => expect(document.querySelector('.inbox-recent [data-key="workspace:dev-1/workspace-1"]')).not.toBeNull());
  });

  it("closes a restored menu state when the reader presses outside it", async () => {
    const { writeUiRecord, readUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const address = uiAddress({ view: "inbox", kind: "menu", sub: "entry" });
    feed([workspace()]);
    unmountInboxList();
    await writeUiRecord(address, { key: "workspace:dev-1/workspace-1" });
    mountInboxList();
    await vi.waitFor(() => expect(rows()[0]?.dataset.key).toBe("workspace:dev-1/workspace-1"));
    expect((await readUiRecord(address)).value.key).toBe("workspace:dev-1/workspace-1");
    await vi.waitFor(async () => {
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      expect((await readUiRecord(address))?.value.key).toBeNull();
    });
  });
  it("paints workspace rows with project and directory context", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1", "workspace:dev-1/workspace-2"]);
    expect(rows()[0].textContent).toContain("Payments");
    expect(rows()[0].textContent).toContain("↑2 ↓1 +8 −3");
  });

  // Captures share anchor order with workspaces on the inbox face and remain
  // outside project blocks on the projects face.
  it("lists an older unrouted capture before a newer workspace", () => {
    feed([workspace()], undefined, [capture()]);

    expect(rows().map((row) => row.dataset.key)).toEqual(["capture:cap-1", "workspace:dev-1/workspace-1"]);
    expect(rows()[0].textContent).toContain("fix the redirect");

    setInboxView("projects");

    expect([...document.querySelectorAll("#inbox-list .inbox-unsorted .inbox-entry")].map((row) => row.dataset.key)).toEqual([
      "capture:cap-1",
    ]);
  });

  it("keeps an archived workspace out when a refreshed feed still carries it", () => {
    feed([workspace(), workspace({ id: "workspace-2", status: "finished" })]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1"]);
    feed([workspace({ status: "finished" }), workspace({ id: "workspace-2", status: "finished" })]);
    expect(rows()).toHaveLength(0);
  });

  it("opens the canonical workspace route, on the machine the row is from", () => {
    feed([workspace()]);
    rows()[0].click();
    expect(navigate).toHaveBeenCalledWith({
      name: "workspace",
      deviceId: "dev-1",
      projectId: "project-1",
      workspaceId: "workspace-1",
      sourceId: "source-api",
      tab: "changes",
    });
  });

  // Done removes the workspace, so the bridge decides when it is offered and
  // says why it is not. The row shows the button either way and hands the
  // reader the bridge's reason.
  it("offers Done when the bridge says so and says why when it does not", () => {
    feed([
      workspace({ can_finish: true, finish_blockers: [] }),
      workspace({ id: "workspace-2", can_finish: false, finish_blockers: ["unpushed"] }),
      workspace({ id: "workspace-3", can_finish: false, finish_blockers: ["dirty"] }),
      workspace({ id: "workspace-4", can_finish: false, finish_blockers: ["agent_working"] }),
      workspace({ id: "workspace-5", can_finish: false, finish_blockers: ["dirty", "unpushed"] }),
      workspace({ id: "workspace-6", can_finish: false, finish_blockers: ["plain_directory"] }),
      workspace({ id: "workspace-7", status: "provisioning", can_finish: false, finish_blockers: [] }),
    ]);
    const done = () => rows().map((row) => row.querySelector("[data-workspace-done]"));
    expect(done().map(Boolean)).toEqual([true, true, true, true, true, true, false]);
    expect(done().map((button) => button && button.disabled)).toEqual([false, true, true, true, true, true, null]);
    expect(done().slice(0, 6).map((button) => button.title)).toEqual([
      "",
      "Push to remote first",
      "Commit or discard changes first",
      "Agent is working",
      "Commit or discard changes first · Push to remote first",
      "Remove the folder that is not a repository first",
    ]);
    expect(done()[0].getAttribute("aria-label")).toBe("Finish workspace Checkout");
  });

  it("shuts Done on a workspace whose bridge sends no verdict at all", () => {
    feed([workspace({ work_summary: { pushes: 0, additions: 0, deletions: 0, clean: true } })]);
    expect(rows()[0].querySelector("[data-workspace-done]").disabled).toBe(true);
  });

  it("marks the workspace named by the route", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Second" })]);
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "project-1", workspaceId: "workspace-2", tab: "changes" };
    inboxListRouteChanged();
    expect(rows().map((row) => row.classList.contains("active"))).toEqual([false, true]);
  });

  it("preserves row elements while applying feed updates", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Second" })]);
    const first = rows()[0];
    feed([workspace({ name: "Checkout updated" }), workspace({ id: "workspace-2", name: "Second" })]);
    expect(rows()[0]).toBe(first);
    expect(rows()[0].textContent).toContain("Checkout updated");
  });
});

describe("a workspace's Done", () => {
  const finishable = { can_finish: true, finish_blockers: [] };

  it("finishes on the machine the row is from, and shuts while it is pending", async () => {
    let resolveFinish;
    workshopCall.mockImplementation((method) =>
      method === "workspace.finish" ? new Promise((done) => { resolveFinish = done; }) : Promise.resolve({}),
    );
    feed([workspace(finishable)]);
    rows()[0].querySelector("[data-workspace-done]").click();
    expect(navigate).not.toHaveBeenCalled();
    expect(workshopCall).toHaveBeenCalledWith("workspace.finish", { workspace_id: "workspace-1" });
    expect(laptopCall).not.toHaveBeenCalled();
    expect(rows()[0].querySelector("[data-workspace-done]").disabled).toBe(true);
    rows()[0].querySelector("[data-workspace-done]").click();
    expect(workshopCall).toHaveBeenCalledTimes(1);
    resolveFinish({});
    await vi.waitFor(() => expect(rows()).toHaveLength(0));
  });

  // Done removes the workspace, so standing in one when it goes leaves the page
  // on nothing. The project it belonged to is where the reader lands.
  it("leaves the workspace it just removed for the project page", async () => {
    App.route = {
      name: "workspace",
      deviceId: "dev-1",
      projectId: "project-1",
      workspaceId: "workspace-1",
      sourceId: "source-api",
      tab: "changes",
    };
    feed([workspace(finishable)]);
    rows()[0].querySelector("[data-workspace-done]").click();
    await vi.waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ name: "project", deviceId: "dev-1", projectId: "project-1" }),
    );
    expect(workshopCall).toHaveBeenCalledWith("workspace.finish", { workspace_id: "workspace-1" });
  });

  it("stays where it is when the workspace it finished is not the open one", async () => {
    App.route = { name: "inbox" };
    feed([workspace(finishable)]);
    rows()[0].querySelector("[data-workspace-done]").click();
    await vi.waitFor(() => expect(rows()).toHaveLength(0));
    expect(navigate).not.toHaveBeenCalled();
  });

  it("restores the row and shows the bridge's words when finishing fails", async () => {
    workshopCall.mockImplementation(async (method) => {
      if (method === "workspace.finish") throw new Error("Workspace has local changes");
      return {};
    });
    feed([workspace(finishable)]);
    rows()[0].querySelector("[data-workspace-done]").click();
    await vi.waitFor(() => expect(rows()[0].querySelector("[data-done-error]").hidden).toBe(false));
    expect(rows()[0].querySelector("[data-done-error]").textContent).toBe("Workspace has local changes");
    expect(rows()[0].querySelector("[data-workspace-done]").disabled).toBe(false);
  });
});

describe("the projects face", () => {
  it("starts a quiet-only block open with a closed Recent count, then paints quiet workspaces and task reasons", async () => {
    const { writeTasksRecord, tasksRecord } = await import("../src/core/trackerCache.js");
    const old = Date.now() - 25 * 60 * 60 * 1000;
    await writeTasksRecord("dev-1", "project-1", tasksRecord([task({
      id: "quiet-task", project_id: "project-1", number: 510, title: "Fold quiet project rows",
      watched: true, assignee: { kind: "user" },
      created_at: new Date(old).toISOString(), updated_at: new Date(old).toISOString(),
    })], []));
    feed([workspace({ session_started_ms: old, last_activity_ms: old })]);
    setInboxView("projects");
    const block = blocks()[0];
    const toggle = () => block.querySelector('[data-recent-toggle="dev-1/project-1"]');
    await vi.waitFor(() => expect(toggle()?.querySelector(".inbox-recent-count").textContent).toBe("2"));
    expect(block.querySelector("[data-project-fold]").getAttribute("aria-expanded")).toBe("true");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(block.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(0);
    expect(document.querySelector('[data-recent-toggle="projects"]')).toBeNull();
    toggle().click();
    await vi.waitFor(() => expect(block.querySelectorAll(".inbox-recent .inbox-entry:not(.inbox-project-agent)")).toHaveLength(2));
    const quietWorkspace = block.querySelector('[data-key="workspace:dev-1/workspace-1"]');
    expect(quietWorkspace.classList.contains("inbox-quiet")).toBe(true);
    expect(quietWorkspace.querySelectorAll(".inbox-line")).toHaveLength(1);
    expect(quietWorkspace.querySelector(".inbox-body > .inbox-facts")).toBeNull();
    expect(quietWorkspace.querySelector(".inbox-facts-float")).not.toBeNull();
    const quietTask = block.querySelector('[data-key="tracker_task:quiet-task"]');
    expect(quietTask.classList.contains("inbox-quiet-task")).toBe(true);
    expect(quietTask.querySelector(".stitle").textContent).toBe("#510 Fold quiet project rows");
    expect(quietTask.querySelector(".inbox-facts").textContent).toBe("Assigned to you");
    expect(block.querySelectorAll(".inbox-project-rows > .inbox-entry:not(.inbox-project-agent)")).toHaveLength(0);
  });

  it("paints live rows before a closed project Recent and keeps it when the last live row goes quiet", async () => {
    const now = Date.now();
    const old = now - 25 * 60 * 60 * 1000;
    const quiet = workspace({ id: "quiet", session_started_ms: old, last_activity_ms: old });
    const live = workspace({ id: "live", session_started_ms: now, last_activity_ms: now });
    feed([quiet, live]);
    setInboxView("projects");
    const block = blocks()[0];
    const section = block.querySelector(":scope > .inbox-recent");
    const toggle = () => section.querySelector("[data-recent-toggle]");
    expect([...block.children].map((child) => child.className)).toEqual([
      "inbox-project-head", "inbox-project-rows", "inbox-recent",
    ]);
    expect(block.querySelector(".inbox-project-rows .inbox-entry").dataset.key).toBe("workspace:dev-1/live");
    expect(toggle().querySelector(".inbox-recent-count").textContent).toBe("1");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(section.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(0);
    feed([quiet, { ...live, session_started_ms: old, last_activity_ms: old }]);
    expect(block.querySelector(":scope > .inbox-recent")).toBe(section);
    expect(toggle().querySelector(".inbox-recent-count").textContent).toBe("2");
    expect(block.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(0);
    toggle().click();
    await vi.waitFor(() => expect(section.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(2));
    feed([quiet, live]);
    expect(toggle().querySelector(".inbox-recent-count").textContent).toBe("1");
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(section.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(1);
    expect(block.querySelector(".inbox-project-rows .inbox-entry").dataset.key).toBe("workspace:dev-1/live");
  });

  it("remembers each quiet-only block's Recent separately from its whole-block fold", async () => {
    const old = Date.now() - 25 * 60 * 60 * 1000;
    feed([
      workspace({ session_started_ms: old, last_activity_ms: old }),
      workspace({ id: "other", project_id: "project-2", session_started_ms: old, last_activity_ms: old }),
    ]);
    setInboxView("projects");
    const toggle = (projectId) => document.querySelector(`[data-recent-toggle="dev-1/${projectId}"]`);
    expect(toggle("project-1")).not.toBeNull();
    toggle("project-1").click();
    await vi.waitFor(() => expect(toggle("project-1").getAttribute("aria-expanded")).toBe("true"));
    expect(toggle("project-2").getAttribute("aria-expanded")).toBe("false");
    blocks()[0].querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(blocks()[0].classList.contains("inbox-folded")).toBe(true));
    unmountInboxList();
    mountInboxList();
    await vi.waitFor(() => expect(blocks()[0].classList.contains("inbox-folded")).toBe(true));
    await vi.waitFor(() => expect(toggle("project-1").getAttribute("aria-expanded")).toBe("true"));
    expect(toggle("project-2").getAttribute("aria-expanded")).toBe("false");
    blocks()[0].querySelector("[data-project-fold]").click();
    await vi.waitFor(() => expect(blocks()[0].classList.contains("inbox-folded")).toBe(false));
    expect(toggle("project-1").getAttribute("aria-expanded")).toBe("true");
    expect(blocks()[0].querySelectorAll(".inbox-recent .inbox-entry:not(.inbox-project-agent)")).toHaveLength(1);
    expect(blocks()[1].querySelectorAll(".inbox-entry:not(.inbox-project-agent)")).toHaveLength(0);
  });

  it("orders project blocks by active streaks while preserving their own Recent", async () => {
    const { writeUiRecord, readUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const recentAddress = uiAddress({ view: "inbox", kind: "fold", sub: "recent" });
    await writeUiRecord(recentAddress, { entries: [["projects", true], ["inbox", false], ["dev-1/project-2", false]] });
    const hour = 60 * 60 * 1000;
    const now = Date.now();
    const session = (time) => ({ session_started_ms: time, last_activity_ms: time });
    const work = [
      workspace(session(now - 2 * hour)),
      workspace({ id: "quiet", project_id: "project-2", ...session(now - 30 * 24 * hour) }),
      workspace({ id: "live", project_id: "project-2", ...session(now - hour) }),
    ];
    const projects = [
      project("project-1", "Zulu", "dev-1", session(now - 2 * hour)),
      project("project-2", "Alpha", "dev-1", session(now - 30 * 24 * hour)),
    ];
    feed(work, projects);
    expect(document.querySelector("#inbox-list > .inbox-recent")).not.toBeNull();
    setInboxView("projects");
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    expect(document.querySelector("#inbox-list > .inbox-recent")).toBeNull();
    expect(document.querySelector('[data-recent-toggle="projects"]')).toBeNull();
    const alpha = blocks()[1];
    const live = alpha.querySelector('[data-key="workspace:dev-1/live"]');
    expect(alpha.querySelector(":scope > .inbox-recent")).not.toBeNull();
    alpha.querySelector('[data-recent-toggle="dev-1/project-2"]').click();
    await vi.waitFor(() => expect(alpha.querySelector('[data-key="workspace:dev-1/quiet"]')).not.toBeNull());
    expect((await readUiRecord(recentAddress)).value.entries.map(([key]) => key)).not.toContain("projects");
    feed(work, [projects[0], { ...projects[1], name: "Zzz" }]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    expect(blocks()[1]).toBe(alpha);
    expect(alpha.querySelector('[data-key="workspace:dev-1/live"]')).toBe(live);
    feed(work.map((row) => row.id === "live" ? { ...row, session_started_ms: now - 3 * hour } : row), projects);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-2", "dev-1/project-1"]);
    expect(blocks()[0]).toBe(alpha);
    expect(alpha.querySelector('[data-key="workspace:dev-1/live"]')).toBe(live);
    setInboxView("inbox");
    expect(document.querySelector('[data-recent-toggle="inbox"]')).not.toBeNull();
    setInboxView("projects");
    expect(document.querySelector("#inbox-list > .inbox-recent")).toBeNull();
    await vi.waitFor(() => expect(document.querySelector('[data-recent-toggle="dev-1/project-2"]').getAttribute("aria-expanded")).toBe("true"));
  });

  it("restores a project fold from cache and repaints an external fold write", async () => {
    const { writeUiRecord } = await import("../src/core/localUiStore.js");
    const address = { deviceId: "", entityId: "", kind: "ui-fold", sub: "inbox:projects" };
    unmountInboxList();
    await writeUiRecord(address, { entries: [["dev-1/project-1", true]] });
    mountInboxList();
    feed([workspace()]);
    setInboxView("projects");
    await vi.waitFor(() => expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(true));
    await writeUiRecord(address, { entries: [["dev-1/project-1", false]] });
    await vi.waitFor(() => expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(false));
  });
  it("keeps project settings and the overflow menu off the inbox head", () => {
    feed([], [project("project-1", "Website", "dev-2")]);
    setInboxView("projects");
    const head = document.querySelector('[data-project="dev-2/project-1"] .inbox-project-head');
    expect(head.querySelector("[data-project-settings]")).toBeNull();
    expect(head.querySelector("[data-menu]")).toBeNull();
    expect(head.querySelector(".inbox-menu")).toBeNull();
    expect(head.querySelector(".inbox-project-hide")).not.toBeNull();
    expect(projectSettings).not.toHaveBeenCalled();
  });

  it("groups workspaces under their projects, by the account-wide project name", () => {
    feed([
      workspace(),
      workspace({ id: "workspace-2", name: "Refunds" }),
      workspace({ id: "workspace-3", project_id: "project-2", name: "Marketing", directories: [] }),
    ]);
    setInboxView("projects");
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    expect(blocks().map((block) => [...block.querySelectorAll(".inbox-entry:not(.inbox-project-agent)")].map((row) => row.dataset.key))).toEqual([
      ["workspace:dev-1/workspace-1", "workspace:dev-1/workspace-2"],
      ["workspace:dev-1/workspace-3"],
    ]);
    expect(blocks().map((block) => block.querySelector(".inbox-project-name").textContent)).toEqual(["Payments", "Website"]);
    expect(document.querySelector("#inbox-list .inbox-new-project")).toBeNull();
    expect(document.querySelectorAll("[data-project-create]")).toHaveLength(2);
    expect(document.getElementById("inbox-list").textContent).not.toMatch(/branch|task/i);
  });

  it("folds a project and preserves workspace row identity across refreshes", async () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Refunds" })]);
    setInboxView("projects");
    const first = rows()[0];
    document.querySelector('[data-project-fold="dev-1/project-1"]').click();
    await vi.waitFor(() => expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(true));
    feed([workspace({ name: "Checkout updated" }), workspace({ id: "workspace-2", name: "Refunds" })]);
    expect(rows()[0]).toBe(first);
    expect(rows()[0].textContent).toContain("Checkout updated");
    expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(true);
  });

  it("keeps the active workspace marked and folds from the project heading", async () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    setInboxView("projects");
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "project-2", workspaceId: "workspace-2", tab: "changes" };
    inboxListRouteChanged();

    expect(rows().map((row) => row.classList.contains("active"))).toEqual([false, true]);
    const block = document.querySelector('[data-project="dev-1/project-2"]');
    expect(block.classList.contains("active")).toBe(true);
    expect(block.querySelector("[data-project-open]")).toBeNull();
    const heading = block.querySelector(".inbox-project-name");
    expect(heading.dataset.projectFold).toBe("dev-1/project-2");
    heading.click();
    await vi.waitFor(() => expect(block.classList.contains("inbox-folded")).toBe(true));
    expect(navigate).not.toHaveBeenCalled();
    heading.click();
    await vi.waitFor(() => expect(block.classList.contains("inbox-folded")).toBe(false));
    block.querySelector('.inbox-project-agent').click();
    expect(navigate).toHaveBeenCalledWith({ name: "project", deviceId: "dev-1", projectId: "project-2" });
  });

  it("keeps a project with no workspaces foldable around its own agent row", async () => {
    feed([workspace()]);
    setInboxView("projects");

    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    const empty = blocks()[1];
    expect(empty.querySelectorAll(".inbox-entry")).toHaveLength(1);
    const agent = empty.querySelector(".inbox-project-agent");
    expect(agent.querySelector(".stitle").textContent).toBe("Website");
    expect(agent.querySelector(".inbox-facts").textContent).toBe("Getting started");
    expect(agent.querySelector(".inbox-project-line")).toBeNull();
    expect(empty.querySelector(".inbox-fold").disabled).toBe(false);
    empty.querySelector(".inbox-project-name").click();
    await vi.waitFor(() => expect(empty.classList.contains("inbox-folded")).toBe(true));
    expect(navigate).not.toHaveBeenCalled();
    expect(empty.querySelector(".inbox-project-agent")).toBe(agent);
  });

  it("creates a workspace in the project named by its group, on that project's machine", () => {
    feed([workspace()]);
    setInboxView("projects");
    document.querySelector('[data-project-create="dev-1/project-2"]').click();
    expect(createWorkspace).toHaveBeenCalledWith(expect.objectContaining({
      projectId: "project-2",
      deviceId: "dev-1",
      projectName: "Website",
      navigate: expect.any(Function),
    }));
    expect(createWorkspace.mock.calls[0][0]).not.toHaveProperty("kind");
  });
});

describe("an account with more than one device", () => {
  const projectsOnBoth = () => [project("project-1", "Payments", "dev-1"), project("project-1", "Payments", "dev-2")];

  const twoDevices = () => {
    feed([workspace(), workspace({ id: "workspace-2", deviceId: "dev-2", name: "Refunds" })], projectsOnBoth());
  };

  /** The same two machines, with a workspace on each the bridge calls clean —
   *  so each row carries the one verb that would ask its own machine. */
  const twoDevicesWithDone = () => {
    const finishable = { can_finish: true, finish_blockers: [] };
    feed(
      [
        workspace(finishable),
        workspace({ id: "workspace-2", deviceId: "dev-2", name: "Refunds", ...finishable }),
      ],
      projectsOnBoth(),
    );
  };

  it("says the machine after a project name two machines share", () => {
    twoDevices();
    expect(rows()[0].textContent).toContain("workshop");
    expect(rows()[1].textContent).toContain("laptop");
  });

  // Greyed is a mark, not a guard: the row keeps the verb it had, and pressing
  // it is what says the machine is away (test/inboxAwayRefusalDom.test.js).
  it("greys a row whose machine is away and leaves its verbs as they were", async () => {
    twoDevicesWithDone();

    setContextOffline("dev-2", { offline: true });

    expect(rows()[1].classList.contains("inbox-offline")).toBe(true);
    expect(rows()[0].classList.contains("inbox-offline")).toBe(false);
    const away = rows()[1].querySelector("[data-workspace-done]");
    const here = rows()[0].querySelector("[data-workspace-done]");
    expect([away.disabled, away.getAttribute("aria-disabled"), away.title]).toEqual([
      here.disabled,
      here.getAttribute("aria-disabled"),
      here.title,
    ]);
    away.click();
    await vi.waitFor(() => expect(rows()[1].querySelector("[data-done-error]").hidden).toBe(false));
    expect(rows()[1].querySelector("[data-done-error]").textContent).toBe(
      "Build cannot archive this workspace because this machine is away.",
    );
    expect(laptopCall).not.toHaveBeenCalledWith("workspace.finish", expect.anything());
  });

  // Grey on its own says "this matters less", not "the machine holding it is
  // not here", so the row says the word too — and stops saying it the moment
  // the machine is back.
  it("wears the word offline while its machine is away", () => {
    twoDevices();

    setContextOffline("dev-2", { offline: true });

    expect(rows()[1].querySelector(".inbox-away").textContent).toBe(deviceOfflineWord);
    expect(rows()[0].querySelector(".inbox-away")).toBeNull();

    setContextOffline("dev-2", { offline: false });

    expect(rows()[1].querySelector(".inbox-away")).toBeNull();
  });

  // A machine whose bridge speaks an API this app cannot read is answering:
  // calling its rows offline would be a lie, and waiting for it would never
  // end. The row says which side is out of date instead.
  it("asks for the update on a row whose bridge this app cannot read", () => {
    twoDevicesWithDone();

    adoptBridgeSelection(contextFor("dev-2"), { version: "0.9.0", unsupported: "bridge" }, null);

    expect(rows()[1].classList.contains("inbox-offline")).toBe(true);
    expect(rows()[1].querySelector(".inbox-away").textContent).toBe("update");
    expect(rows()[1].querySelector("[data-workspace-done]").disabled).toBe(false);
    expect(rows()[0].querySelector(".inbox-away")).toBeNull();
  });

  it("greys a block whose machine is away and leaves its + and Hide live", () => {
    twoDevices();
    setInboxView("projects");

    setContextOffline("dev-2", { offline: true });

    const away = document.querySelector('[data-project="dev-2/project-1"]');
    expect(away.classList.contains("inbox-offline")).toBe(true);
    const here = document.querySelector('[data-project="dev-1/project-1"]');
    expect(here.classList.contains("inbox-offline")).toBe(false);
    for (const control of ["[data-project-create]", "[data-project-hide]"]) {
      const shown = away.querySelector(control);
      expect(shown.hasAttribute("disabled")).toBe(false);
      expect(shown.hasAttribute("aria-disabled")).toBe(false);
      expect(shown.title).toBe(here.querySelector(control).title);
    }
    away.querySelector("[data-project-create]").click();
    expect(createWorkspace).toHaveBeenCalledWith(expect.objectContaining({ projectId: "project-1", deviceId: "dev-2" }));
  });

  // A block whose machine cannot be asked anything says why it is inert, and
  // says it whether or not another machine shares the name: "which laptop" is
  // not the reader's question once none of them can answer.
  it("says Offline on a block whose machine is away, and on one nothing names", () => {
    twoDevices();
    setInboxView("projects");

    setContextOffline("dev-2", { offline: true });

    const tagOf = (projectKey) =>
      document.querySelector(`[data-project="${projectKey}"] .inbox-project-device`).textContent.trim();
    expect(tagOf("dev-2/project-1")).toBe("Offline");
    expect(tagOf("dev-1/project-1")).toBe("workshop");

    // A machine the account's device list has never heard of can answer for
    // nothing either, so its block reads the same way.
    App.devices = App.devices.filter((device) => device.id !== "dev-2");
    setContextOffline("dev-2", { offline: false });
    twoDevices();
    expect(tagOf("dev-2/project-1")).toBe("Offline");
  });

  // Which controls a block has is what the cache holds: Hide is in every
  // block's head, whether its machine is answering, connecting or gone.
  it("offers Hide directly in every block's head, whatever its machine is doing", () => {
    let tick = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => tick++);
    try {
      twoDevices();
      setInboxView("projects");
      const hides = () => [...document.querySelectorAll(".inbox-project-head .inbox-project-hide")];
      expect(hides().map((item) => item.dataset.projectHide)).toEqual(["dev-2/project-1", "dev-1/project-1"]);

      setContextOffline("dev-2", { offline: true });
      expect(hides().map((item) => item.dataset.projectHide)).toEqual(["dev-2/project-1", "dev-1/project-1"]);
      expect(hides().every((item) => !item.disabled)).toBe(true);

      setContextOffline("dev-2", { offline: false });
      expect(hides().map((item) => item.dataset.projectHide)).toEqual(["dev-2/project-1", "dev-1/project-1"]);
    } finally {
      clock.mockRestore();
    }
  });

  // Hide drops the project from the cache — which, on a machine that has gone,
  // is everything it is. The block and its rows leave with it; the other
  // machine's project of the same number is untouched.
  it("takes the block and its rows off the rail when Hide is pressed", async () => {
    twoDevices();
    setInboxView("projects");
    setContextOffline("dev-2", { offline: true });

    document.querySelector('[data-project-hide="dev-2/project-1"]').click();

    expect(hidden).toEqual([{ deviceId: "dev-2", projectKey: "dev-2/project-1" }]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1"]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1"]);
    expect(navigate).not.toHaveBeenCalled();
  });

  // Hide is a native button in the tab order. jsdom does not perform trusted
  // keyboard activation, so dispatch the key to check it is left to the
  // browser, then click as the browser does for Enter or Space.
  it.each(["Enter", " "])("keeps direct Hide focusable and lets %j activate it", (key) => {
    twoDevices();
    setInboxView("projects");
    setContextOffline("dev-2", { offline: true });
    const hide = document.querySelector('[data-project-hide="dev-2/project-1"]');
    expect(hide.tagName).toBe("BUTTON");
    expect(hide.tabIndex).toBe(0);
    expect(hide.getAttribute("role")).toBeNull();
    hide.focus();
    expect(document.activeElement).toBe(hide);
    expect(hide.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }))).toBe(true);
    hide.click();
    expect(hidden).toEqual([{ deviceId: "dev-2", projectKey: "dev-2/project-1" }]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1"]);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("narrows the list to one machine without touching the route", async () => {
    twoDevices();
    const standing = App.route;
    await rememberDeviceFilter("dev-2");
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-2/workspace-2"]);
    expect(App.route).toBe(standing);
    await rememberDeviceFilter(null);
    expect(rows()).toHaveLength(2);
  });

  // The answer to a fresh project.create is not a feed row: it carries the id
  // one bridge minted and no machine at all. Left unstamped the link is
  // device-less, and a device-less link is resolved by asking every machine —
  // which hands the reader whichever one happens to hold the same numbered
  // project. Creation already knows the machine it asked, so the route says it.
  //
  // And it opens the new project's own PAGE, never that project's checkout: a
  // project is the template its workspaces are cut from, and the page is where
  // the first one is made.
  it("opens a new project's page on the machine it was made on, not its checkout", () => {
    App.selectedDeviceId = "dev-1";
    App.deviceFilter = "dev-1";
    twoDevices();

    openNewProject();
    const [done, options] = newRepoSheet.mock.calls[0];
    expect(options.defaultDeviceId).toBe("dev-1");
    done({ project_id: "project-1", base_branch: "main" }, { id: "dev-2", name: "laptop" });

    expect(navigate).toHaveBeenCalledWith({ name: "project", deviceId: "dev-2", projectId: "project-1" });
  });

  it("defaults project creation to the sidebar device instead of the remembered home device", () => {
    App.selectedDeviceId = "dev-1";
    App.deviceFilter = "dev-2";
    twoDevices();

    openNewProject();

    expect(newRepoSheet.mock.calls[0][1].defaultDeviceId).toBe("dev-2");
  });

  it("leaves the project device unselected when the sidebar shows all devices", () => {
    App.selectedDeviceId = "dev-1";
    App.deviceFilter = null;
    twoDevices();

    openNewProject();

    expect(newRepoSheet.mock.calls[0][1].defaultDeviceId).toBeNull();
  });
});

// ---- captures ----------------------------------------------------------------
//
// A capture on the rail is a route in progress: unfinished business the router
// has not placed yet. The row is where that routing is made visible and
// reversible — it opens the page that decides, retries a route that gave up,
// and sends the capture somewhere else — and everything it asks, it asks the
// machine holding the capture.

const captureRowFor = (id) => document.querySelector(`.capture-entry[data-capture="${id}"]`);
const waitForRerouteMenu = async () => {
  await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector(".reroute-menu")).not.toBeNull());
  return captureRowFor("cap-1").querySelector(".reroute-menu");
};
const flush = () => new Promise((done) => setTimeout(done, 0));

/** A branch the feed still carries. The rail no longer lists branches, but the
 *  reroute picker still offers the ones a project already has. */
const branchRow = (over = {}) => {
  const base = { kind: "branch", deviceId: "dev-1", project_id: "project-1", branch: "build/login", run_id: "run-1", ...over };
  return { ...base, projectKey: key(base.deviceId, base.project_id) };
};

/** A capture the router has placed, which opens where it put it. */
const routedCapture = (over = {}) =>
  capture({
    state: "routed",
    project_id: "project-1",
    project: "Payments",
    branch: "build/login",
    routing: { project_id: "project-1", kind: "branch", target_id: "build/login" },
    ...over,
  });

/** A capture whose route gave up: the one state that carries a retry. */
const failedCapture = (over = {}) =>
  capture({ state: "failed", unread: true, unread_count: 1, unread_reason: "routing_failed", ...over });

describe("captures on the rail", () => {
  it("restores and redraws the destination picker from real cache records", async () => {
    const { writeUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const address = uiAddress({ view: "inbox-captures", kind: "menu" });
    unmountInboxList();
    await writeUiRecord(address, { key: "capture:cap-1", branchProject: null });
    mountInboxList();
    feed([], undefined, [routedCapture()]);
    await waitForRerouteMenu();

    await writeUiRecord(address, { key: null, branchProject: null });
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector(".reroute-menu")).toBeNull());
  });

  // A capture leaves the inbox by being routed, so there is nothing to clear —
  // and no entity to clear it on.
  it("offers no way to clear a capture", () => {
    feed([workspace()], undefined, [capture()]);
    const row = captureRowFor("cap-1");
    expect(row.querySelector("[data-dismiss]")).toBeNull();
    expect(row.querySelector("[data-menu]")).toBeNull();
  });

  it("opens the decision page for a capture the router is still deciding", async () => {
    feed([], undefined, [capture()]);
    expect(captureRowFor("cap-1").textContent).toContain("Deciding where this goes");
    captureRowFor("cap-1").click();
    await flush();
    expect(navigate).toHaveBeenCalledWith({ name: "capture", id: "cap-1" });
    // A capture holds no conversation, so there is nothing to read through.
    expect(workshopCall).not.toHaveBeenCalledWith("entity.seen", expect.anything());
  });

  // Answering the router is a decision, not a text field wedged into a row:
  // the row is the conversation entry, and it opens the page that decides.
  it("opens the decision page for a capture the router is asking about", async () => {
    feed([], undefined, [
      capture({
        state: "unrouted",
        unread: true,
        unread_count: 1,
        unread_reason: "router_question",
        question: { text: "Which project?", asked_at: "t", answer: null },
      }),
    ]);
    const row = captureRowFor("cap-1");
    expect(row.textContent).toContain("Which project?");
    expect(row.querySelector("[data-capture-answer]")).toBeNull();
    row.click();
    await flush();
    expect(navigate).toHaveBeenCalledWith({ name: "capture", id: "cap-1" });
    expect(workshopCall).not.toHaveBeenCalledWith("capture.answer", expect.anything());
  });

  it("opens a routed capture where it was routed, on its own machine", async () => {
    feed([], undefined, [routedCapture()]);
    captureRowFor("cap-1").click();
    await flush();
    expect(navigate).toHaveBeenCalledWith({
      name: "branch",
      deviceId: "dev-1",
      projectId: "project-1",
      branch: "build/login",
      tab: "changes",
    });
  });

  it("re-fires the router on a route that gave up", async () => {
    feed([], undefined, [failedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-retry]").click();
    await flush();
    expect(workshopCall).toHaveBeenCalledWith("capture.reroute", { capture_id: "cap-1" });
    expect(laptopCall).not.toHaveBeenCalled();
  });

  it("retries one failed route while another retry is still in flight", async () => {
    workshopCall.mockImplementation((method) => (method === "capture.reroute" ? new Promise(() => {}) : Promise.resolve({})));
    feed([], undefined, [failedCapture(), failedCapture({ capture_id: "cap-2" })]);

    captureRowFor("cap-1").querySelector("[data-capture-retry]").click();
    await flush();
    captureRowFor("cap-2").querySelector("[data-capture-retry]").click();
    await flush();

    const rerouted = workshopCall.mock.calls.filter(([method]) => method === "capture.reroute");
    expect(rerouted.map(([, params]) => params.capture_id)).toEqual(["cap-1", "cap-2"]);
  });

  it("says on the row when a reroute is refused", async () => {
    workshopCall.mockImplementation(async (method) => {
      if (method === "capture.reroute") throw new Error("unknown project_id: project-9");
      return {};
    });
    feed([], undefined, [failedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-retry]").click();
    await vi.waitFor(() => expect(captureRowFor("cap-1").querySelector("[data-capture-error]").hidden).toBe(false));
    expect(captureRowFor("cap-1").querySelector("[data-capture-error]").textContent).toContain("unknown project_id");
  });

  it("sends a capture somewhere else through the picker on its row", async () => {
    feed([], undefined, [routedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    const picker = await waitForRerouteMenu();
    expect([...picker.querySelectorAll(".reroute-project .mt")].map((name) => name.textContent)).toEqual([
      "Payments",
      "Website",
    ]);

    picker.querySelector('[data-reroute-branch-open="project-2"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());
    captureRowFor("cap-1").querySelector('[data-reroute-project="project-2"][data-reroute-kind="branch"]').click();
    await flush();

    // A branch left unnamed is the daemon naming it after what was said.
    expect(workshopCall).toHaveBeenCalledWith("capture.reroute", {
      capture_id: "cap-1",
      project_id: "project-2",
      kind: "branch",
    });
  });

  it("names the branch it is rerouted to, offering the ones the project has", async () => {
    feed([], undefined, [branchRow(), routedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    await waitForRerouteMenu();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());

    const field = captureRowFor("cap-1").querySelector("[data-reroute-branch]");
    expect([...captureRowFor("cap-1").querySelectorAll("#reroute-branches option")].map((option) => option.value)).toEqual([
      "build/login",
    ]);

    field.value = "build/csv-export";
    captureRowFor("cap-1").querySelector('[data-reroute-project="project-1"][data-reroute-kind="branch"]').click();
    await flush();
    expect(workshopCall).toHaveBeenCalledWith("capture.reroute", {
      capture_id: "cap-1",
      project_id: "project-1",
      kind: "branch",
      branch: "build/csv-export",
    });
  });

  it.each([false, true])("clears a submitted branch after unmount while preserving newer drafts (%s)", async (newer) => {
    let finish;
    const accepted = new Promise((resolve) => { finish = resolve; });
    workshopCall.mockImplementation((method) => method === "capture.reroute" ? accepted : Promise.resolve({}));
    feed([], undefined, [routedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    await waitForRerouteMenu();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());
    const field = captureRowFor("cap-1").querySelector("[data-reroute-branch]");
    field.value = "build/sent-draft";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    captureRowFor("cap-1").querySelector('[data-reroute-project="project-1"][data-reroute-kind="branch"]').click();
    await vi.waitFor(() => expect(workshopCall).toHaveBeenCalledWith("capture.reroute", {
      capture_id: "cap-1", project_id: "project-1", kind: "branch", branch: "build/sent-draft",
    }));
    const { readUiRecord, writeUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const draftAddress = uiAddress({ deviceId: "dev-1", entityId: "cap-1", view: "inbox-capture-branch",
      kind: "draft", sub: "project-1" });
    await vi.waitFor(async () => expect((await readUiRecord(draftAddress))?.value.branch).toBe("build/sent-draft"));
    unmountInboxList();
    if (newer) await writeUiRecord(draftAddress, { branch: "build/newer-draft" });
    finish({ id: "cap-1", text: "captured work", state: "routed", routing: {
      project_id: "project-1", kind: "branch", target_id: "build/sent-draft",
    } });
    await vi.waitFor(async () => expect((await readUiRecord(draftAddress))?.value.branch)
      .toBe(newer ? "build/newer-draft" : ""));
  });

  it("reorders live while preserving the focused branch box and selection", async () => {
    feed([], undefined, [routedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    await waitForRerouteMenu();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());

    const field = captureRowFor("cap-1").querySelector("[data-reroute-branch]");
    field.focus();
    field.value = "build/csv";
    field.setSelectionRange(6, 9, "backward");

    feed([workspace()], undefined, [routedCapture()]); // a tick with something new to say
    expect(captureRowFor("cap-1").querySelector("[data-reroute-branch]")).toBe(field);
    expect(rows().map((row) => row.dataset.key)).toEqual(["capture:cap-1", "workspace:dev-1/workspace-1"]);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("build/csv");
    expect([field.selectionStart, field.selectionEnd, field.selectionDirection]).toEqual([6, 9, "backward"]);

    // The capture itself now has to move across the existing workspace.
    const older = Date.now() - 2 * 60 * 60 * 1000;
    feed([workspace({ session_started_ms: older, last_activity_ms: Date.now() })], undefined, [routedCapture()]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1", "capture:cap-1"]);
    expect(captureRowFor("cap-1").querySelector("[data-reroute-branch]")).toBe(field);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("build/csv");
    expect([field.selectionStart, field.selectionEnd, field.selectionDirection]).toEqual([6, 9, "backward"]);
  });

  it("moves project blocks live while a capture branch field has focus", async () => {
    const now = Date.now();
    const first = workspace({ session_started_ms: now - 4 * 3600_000, last_activity_ms: now });
    const second = workspace({ id: "workspace-2", project_id: "project-2",
      session_started_ms: now - 2 * 3600_000, last_activity_ms: now });
    feed([first, second], undefined, [routedCapture()]);
    setInboxView("projects");
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    await waitForRerouteMenu();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());
    const field = captureRowFor("cap-1").querySelector("[data-reroute-branch]");
    field.focus();
    field.value = "build/csv";
    field.setSelectionRange(6, 9);
    const kept = blocks();

    feed([{ ...first, session_started_ms: now }, second], undefined, [routedCapture()]);
    expect(blocks()).toEqual([...kept].reverse());
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("build/csv");
    expect([field.selectionStart, field.selectionEnd]).toEqual([6, 9]);
  });
});

describe("a capture on another device", () => {
  // A reroute goes to the machine holding the capture, and that daemon knows
  // only the projects it minted itself — every machine has a `project-1`. So
  // the picker offers that device's projects and the branches they already have.
  it("offers its own device's projects when its capture is rerouted", async () => {
    const theirProject = project("project-1", "their notes", "dev-2");
    const theirCapture = routedCapture({ deviceId: "dev-2", projectKey: key("dev-2", "project-1"), project: "their notes" });
    const theirBranch = branchRow({ deviceId: "dev-2", branch: "build/away", run_id: "run-2" });
    const mine = { items: [branchRow()], projects: [project("project-1", "Payments")], workspaces: [] };
    const theirs = { items: [theirBranch, theirCapture], projects: [theirProject], workspaces: [] };
    feed([], [...mine.projects, ...theirs.projects], [...mine.items, ...theirs.items], { "dev-1": mine, "dev-2": theirs });

    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    const picker = await waitForRerouteMenu();
    expect([...picker.querySelectorAll(".reroute-project .mt")].map((name) => name.textContent)).toEqual(["their notes"]);

    picker.querySelector('[data-reroute-branch-open="project-1"]').click();
    await vi.waitFor(() => expect(captureRowFor("cap-1")?.querySelector("[data-reroute-branch]")).not.toBeNull());
    expect([...captureRowFor("cap-1").querySelectorAll("#reroute-branches option")].map((option) => option.value)).toEqual([
      "build/away",
    ]);
  });

  it("sends the reroute to the machine holding the capture, not the home one", async () => {
    const theirCapture = failedCapture({ deviceId: "dev-2", projectKey: key("dev-2", "project-1") });
    feed([], undefined, [theirCapture]);
    captureRowFor("cap-1").querySelector("[data-capture-retry]").click();
    await flush();
    expect(laptopCall).toHaveBeenCalledWith("capture.reroute", { capture_id: "cap-1" });
    expect(workshopCall).not.toHaveBeenCalledWith("capture.reroute", expect.anything());
  });

  // A capture this client has just sent is on the rail before any device's feed
  // carries it, and it is on exactly one machine: the one creation goes to. The
  // picker narrows the rail to one machine, so it reaches that row like every
  // other.
  it("the filter hides a capture this client is still holding", async () => {
    const host = document.createElement("div");
    host.id = "compose";
    document.getElementById("inbox-rail").insertBefore(host, document.getElementById("inbox-list"));
    const { initCompose } = await import("../src/core/composeView.js");
    const record = { id: "cap-9", text: "ship it", created_at: "2026-09-02T12:00:00Z", state: "routing", routing: null };
    workshopCall.mockImplementation(async (method) => (method.startsWith("capture.") ? record : {}));
    initCompose();

    document.querySelector("#compose-open").click();
    document.querySelector("#compose-text").value = "ship it";
    document.querySelector("#compose-send").click();
    await vi.waitFor(() => expect(captureRowFor("cap-9")).toBeTruthy());

    await rememberDeviceFilter("dev-2");
    expect(captureRowFor("cap-9")).toBeNull();

    await rememberDeviceFilter("dev-1");
    expect(captureRowFor("cap-9")).toBeTruthy();
    await rememberDeviceFilter(null);
  });

  it("releases a late capture repaint when the rail unmounts", async () => {
    const host = document.createElement("div");
    host.id = "compose";
    document.getElementById("inbox-rail").insertBefore(host, document.getElementById("inbox-list"));
    const { initCompose, forgetCaptureRecord } = await import("../src/core/composeView.js");
    const record = { id: "cap-late", text: "ship it", created_at: "2026-09-02T12:00:00Z", state: "routing", routing: null };
    workshopCall.mockResolvedValue(record);
    initCompose();
    document.querySelector("#compose-open").click();
    document.querySelector("#compose-text").value = "ship it";
    document.querySelector("#compose-send").click();
    await vi.waitFor(() => expect(captureRowFor("cap-late")).toBeTruthy());

    unmountInboxList();
    const savedDocument = globalThis.document;
    try {
      globalThis.document = undefined;
      expect(() => forgetCaptureRecord("cap-late")).not.toThrow();
    } finally {
      globalThis.document = savedDocument;
    }
  });
});
