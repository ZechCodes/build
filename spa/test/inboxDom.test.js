// @vitest-environment jsdom
// The rail, painted from every device's workspaces at once.
//
// A workspace belongs to one machine, so every row carries the machine that
// answered for it and the account-wide names minted from it
// (core/deviceKey.js): a row's verbs go to its own device, a row whose machine
// is away is greyed with its verbs shut, and the picker narrows the list
// without touching the route.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { deviceOfflineMark, deviceOfflineWord } from "../src/core/text.js";

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
const project = (id, name, deviceId = "dev-1") => ({
  id,
  project_id: id,
  name,
  deviceId,
  projectKey: key(deviceId, id),
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
  created_at: "2026-09-02T12:00:00Z",
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

const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
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
  it("paints workspace rows with project and directory context", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1", "workspace:dev-1/workspace-2"]);
    expect(rows()[0].textContent).toContain("Payments");
    expect(rows()[0].textContent).toContain("↑2 ↓1 +8 −3");
  });

  // A capture is unfinished business that belongs to no project yet — the
  // router has not said where it goes — so it stands above the workspaces
  // rather than under any of them, on both faces.
  it("lists a capture nothing has routed yet above the workspaces", () => {
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
  it("opens settings on the owning device and leaves a deleted project's route", async () => {
    feed([], [project("project-1", "Website", "dev-2")]);
    setInboxView("projects");
    document.querySelector('[data-project-settings="dev-2/project-1"]').click();
    expect(projectSettings).toHaveBeenCalledWith("project-1", expect.any(Object));
    const options = projectSettings.mock.calls[0][1];
    await options.callRpc("project.list");
    expect(laptopCall).toHaveBeenCalledWith("project.list");
    expect(workshopCall).not.toHaveBeenCalled();
    App.route = { name: "workspace", deviceId: "dev-2", projectId: "project-1", workspaceId: "workspace-1" };
    await options.onDeleted();
    expect(navigate).toHaveBeenCalledWith({ name: "inbox" });
    expect(refreshFeed).toHaveBeenCalledWith("dev-2");
  });

  it("groups workspaces under their projects, by the account-wide project name", () => {
    feed([
      workspace(),
      workspace({ id: "workspace-2", name: "Refunds" }),
      workspace({ id: "workspace-3", project_id: "project-2", name: "Marketing", directories: [] }),
    ]);
    setInboxView("projects");
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    expect(blocks().map((block) => [...block.querySelectorAll(".inbox-entry")].map((row) => row.dataset.key))).toEqual([
      ["workspace:dev-1/workspace-1", "workspace:dev-1/workspace-2"],
      ["workspace:dev-1/workspace-3"],
    ]);
    expect(blocks().map((block) => block.querySelector(".inbox-project-name").textContent)).toEqual(["Payments", "Website"]);
    expect(document.querySelector("#inbox-list .inbox-new-project")).toBeNull();
    expect(document.querySelectorAll("[data-project-create]")).toHaveLength(2);
    expect(document.getElementById("inbox-list").textContent).not.toMatch(/branch|issue/i);
  });

  it("folds a project and preserves workspace row identity across refreshes", () => {
    feed([workspace(), workspace({ id: "workspace-2", name: "Refunds" })]);
    setInboxView("projects");
    const first = rows()[0];
    document.querySelector('[data-project-fold="dev-1/project-1"]').click();
    expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(true);
    feed([workspace({ name: "Checkout updated" }), workspace({ id: "workspace-2", name: "Refunds" })]);
    expect(rows()[0]).toBe(first);
    expect(rows()[0].textContent).toContain("Checkout updated");
    expect(document.querySelector('[data-project="dev-1/project-1"]').classList.contains("inbox-folded")).toBe(true);
  });

  it("keeps the active workspace marked in its project and opens the project from its head", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    setInboxView("projects");
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "project-2", workspaceId: "workspace-2", tab: "changes" };
    inboxListRouteChanged();

    expect(rows().map((row) => row.classList.contains("active"))).toEqual([false, true]);
    expect(document.querySelector('[data-project="dev-1/project-2"]').classList.contains("active")).toBe(true);

    // The head is the project, not a workspace inside it: it opens the
    // project's own page, which is where the block's workspaces are listed.
    document.querySelector('[data-project-open="dev-1/project-2"]').click();
    expect(navigate).toHaveBeenCalledWith({ name: "project", deviceId: "dev-1", projectId: "project-2" });
  });

  // A project with no workspaces is still one of the account's projects, so it
  // keeps its block — with nothing to fold, and the chevron shut rather than
  // missing, so the heads stay in line.
  it("blocks a project that has no workspaces, with nothing to fold", () => {
    feed([workspace()]);
    setInboxView("projects");

    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1", "dev-1/project-2"]);
    expect(blocks()[1].querySelectorAll(".inbox-entry")).toHaveLength(0);
    expect(blocks()[1].querySelector("[data-project-fold]").disabled).toBe(true);
    expect(blocks()[0].querySelector("[data-project-fold]").disabled).toBe(false);
    // Empty or not, its head opens the project: there is a page for it.
    blocks()[1].querySelector("[data-project-open]").click();
    expect(navigate).toHaveBeenCalledWith({ name: "project", deviceId: "dev-1", projectId: "project-2" });
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

  it("greys a row whose machine is away and shuts the verbs that would ask it", () => {
    twoDevicesWithDone();

    setContextOffline("dev-2", { offline: true });

    expect(rows()[1].classList.contains("inbox-offline")).toBe(true);
    expect(rows()[0].classList.contains("inbox-offline")).toBe(false);
    // Greyed is not shut: the verb that would ask the machine says why instead.
    const away = rows()[1].querySelector("[data-workspace-done]");
    expect(away.hasAttribute("disabled")).toBe(true);
    expect(away.title).toBe(deviceOfflineMark);
    expect(rows()[0].querySelector("[data-workspace-done]").hasAttribute("disabled")).toBe(false);
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
    expect(rows()[1].querySelector("[data-workspace-done]").title).toBe("Bridge is out of date");
    expect(rows()[0].querySelector(".inbox-away")).toBeNull();
  });

  it("greys a block whose machine is away and shuts its +", () => {
    twoDevices();
    setInboxView("projects");

    setContextOffline("dev-2", { offline: true });

    const away = document.querySelector('[data-project="dev-2/project-1"]');
    expect(away.classList.contains("inbox-offline")).toBe(true);
    const create = away.querySelector("[data-project-create]");
    expect(create.hasAttribute("disabled")).toBe(true);
    expect(create.title).toBe(deviceOfflineMark);
    const here = document.querySelector('[data-project="dev-1/project-1"]');
    expect(here.classList.contains("inbox-offline")).toBe(false);
    expect(here.querySelector("[data-project-create]").hasAttribute("disabled")).toBe(false);
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

  it("offers Hide only on a block whose machine is away", () => {
    twoDevices();
    setInboxView("projects");
    expect(document.querySelectorAll("[data-project-hide]")).toHaveLength(0);

    setContextOffline("dev-2", { offline: true });

    expect([...document.querySelectorAll("[data-project-hide]")].map((button) => button.dataset.projectHide)).toEqual([
      "dev-2/project-1",
    ]);
    // Everything else on an away block is shut with the reason; hide is the one
    // thing that can still be done, so it stays live.
    const hide = document.querySelector("[data-project-hide]");
    expect(hide.hasAttribute("disabled")).toBe(false);

    setContextOffline("dev-2", { offline: false });
    expect(document.querySelectorAll("[data-project-hide]")).toHaveLength(0);
  });

  // Hide drops the project from the cache — which, on a machine that has gone,
  // is everything it is. The block and its rows leave with it; the other
  // machine's project of the same number is untouched.
  it("takes the block and its rows off the rail when Hide is pressed", () => {
    twoDevices();
    setInboxView("projects");
    setContextOffline("dev-2", { offline: true });

    document.querySelector("[data-project-hide]").click();

    expect(hidden).toEqual([{ deviceId: "dev-2", projectKey: "dev-2/project-1" }]);
    expect(blocks().map((block) => block.dataset.project)).toEqual(["dev-1/project-1"]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1"]);
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
    await flush();

    const picker = captureRowFor("cap-1").querySelector(".reroute-menu");
    expect([...picker.querySelectorAll(".reroute-project .mt")].map((name) => name.textContent)).toEqual([
      "Payments",
      "Website",
    ]);

    picker.querySelector('[data-reroute-branch-open="project-2"]').click();
    await flush();
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
    await flush();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await flush();

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

  // The list is rewritten whole on every feed tick, and naming a branch is
  // typing into a box that lives in it.
  it("holds the feed off the branch box while it is being typed into", async () => {
    feed([], undefined, [routedCapture()]);
    captureRowFor("cap-1").querySelector("[data-capture-reroute]").click();
    await flush();
    captureRowFor("cap-1").querySelector('[data-reroute-branch-open="project-1"]').click();
    await flush();

    const field = captureRowFor("cap-1").querySelector("[data-reroute-branch]");
    field.focus();
    field.value = "build/csv";

    feed([workspace()], undefined, [routedCapture()]); // a tick with something new to say
    expect(captureRowFor("cap-1").querySelector("[data-reroute-branch]")).toBe(field);
    expect(rows().map((row) => row.dataset.key)).toEqual(["capture:cap-1"]); // held back while typing

    field.blur();
    feed([workspace()], undefined, [routedCapture()]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["capture:cap-1", "workspace:dev-1/workspace-1"]);
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
    await flush();
    const picker = captureRowFor("cap-1").querySelector(".reroute-menu");
    expect([...picker.querySelectorAll(".reroute-project .mt")].map((name) => name.textContent)).toEqual(["their notes"]);

    picker.querySelector('[data-reroute-branch-open="project-1"]').click();
    await flush();
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
