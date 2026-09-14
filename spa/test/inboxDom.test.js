// @vitest-environment jsdom
// The rail, painted from every device's workspaces at once.
//
// A workspace belongs to one machine, so every row carries the machine that
// answered for it and the account-wide names minted from it
// (core/deviceKey.js): a row's verbs go to its own device, a row whose machine
// is away is greyed with its verbs shut, and the picker narrows the list
// without touching the route.
import { beforeEach, describe, expect, it, vi } from "vitest";
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
const refreshFeed = vi.fn(async () => {});

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
  primaryRunIdFor: () => null,
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));
vi.mock("../src/core/inboxShell.js", () => ({ goFromInbox: (...args) => navigate(...args) }));
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => createWorkspace(...args) }));

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
    work_summary: { pushes: 2, additions: 8, deletions: 3 },
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

const feed = (
  workspaces,
  projects = [project("project-1", "Payments"), project("project-2", "Website")],
  items = [],
) => {
  snapshot = { items, pending: [], projects, workspaces, devices: { "dev-1": { items, projects, workspaces } } };
  deliver();
};

const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const blocks = () => [...document.querySelectorAll("#inbox-list .inbox-project")];

let App;
let mountInboxList;
let inboxListRouteChanged;
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
  navigate.mockReset();
  createWorkspace.mockReset();
  refreshFeed.mockClear();
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  ({ App } = await import("../src/app.js"));
  ({ mountInboxList, inboxListRouteChanged, setInboxView } = await import("../src/core/inboxView.js"));
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

describe("the workspace inbox", () => {
  it("paints workspace rows with project and directory context", () => {
    feed([workspace(), workspace({ id: "workspace-2", project_id: "project-2", name: "Marketing", directories: [] })]);
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-1/workspace-1", "workspace:dev-1/workspace-2"]);
    expect(rows()[0].textContent).toContain("Payments");
    expect(rows()[0].textContent).toContain("2 pushes · +8 −3");
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

  it("offers Done only for a workspace the bridge reports clean", () => {
    feed([
      workspace({ work_summary: { pushes: 0, additions: 0, deletions: 0, clean: true } }),
      workspace({ id: "workspace-2", work_summary: { pushes: 0, additions: 0, deletions: 0, clean: false } }),
      workspace({ id: "workspace-3", work_summary: { pushes: 0, additions: 0, deletions: 0 } }),
      workspace({ id: "workspace-4", status: "provisioning", work_summary: { pushes: 0, additions: 0, deletions: 0, clean: true } }),
    ]);
    expect(rows().map((row) => Boolean(row.querySelector("[data-workspace-done]")))).toEqual([true, false, false, false]);
    expect(rows()[0].querySelector("[data-workspace-done]").getAttribute("aria-label")).toBe("Archive workspace Checkout");
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
  const clean = { pushes: 0, additions: 0, deletions: 0, clean: true };

  it("finishes on the machine the row is from, and shuts while it is pending", async () => {
    let resolveFinish;
    workshopCall.mockImplementation((method) =>
      method === "workspace.finish" ? new Promise((done) => { resolveFinish = done; }) : Promise.resolve({}),
    );
    feed([workspace({ work_summary: clean })]);
    rows()[0].querySelector("[data-workspace-done]").click();
    expect(navigate).not.toHaveBeenCalled();
    expect(workshopCall).toHaveBeenCalledWith("workspace.finish", { workspace_id: "workspace-1", require_clean: true });
    expect(laptopCall).not.toHaveBeenCalled();
    expect(rows()[0].querySelector("[data-workspace-done]").disabled).toBe(true);
    rows()[0].querySelector("[data-workspace-done]").click();
    expect(workshopCall).toHaveBeenCalledTimes(1);
    resolveFinish({});
    await vi.waitFor(() => expect(rows()).toHaveLength(0));
  });

  it("restores the row and shows the bridge's words when finishing fails", async () => {
    workshopCall.mockImplementation(async (method) => {
      if (method === "workspace.finish") throw new Error("Workspace has local changes");
      return {};
    });
    feed([workspace({ work_summary: clean })]);
    rows()[0].querySelector("[data-workspace-done]").click();
    await vi.waitFor(() => expect(rows()[0].querySelector("[data-done-error]").hidden).toBe(false));
    expect(rows()[0].querySelector("[data-done-error]").textContent).toBe("Workspace has local changes");
    expect(rows()[0].querySelector("[data-workspace-done]").disabled).toBe(false);
  });
});

describe("the projects face", () => {
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
    const clean = { pushes: 0, additions: 0, deletions: 0, clean: true };
    feed(
      [
        workspace({ work_summary: clean }),
        workspace({ id: "workspace-2", deviceId: "dev-2", name: "Refunds", work_summary: clean }),
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

  it("narrows the list to one machine without touching the route", () => {
    twoDevices();
    const standing = App.route;
    rememberDeviceFilter("dev-2");
    expect(rows().map((row) => row.dataset.key)).toEqual(["workspace:dev-2/workspace-2"]);
    expect(App.route).toBe(standing);
    rememberDeviceFilter(null);
    expect(rows()).toHaveLength(2);
  });
});
