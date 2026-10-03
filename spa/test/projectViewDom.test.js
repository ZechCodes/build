/** @vitest-environment jsdom */
// The project's own surface: what the project holds in the main pane, and who
// you talk to about the project in the rail.
//
// A project is a template — its checkout is what workspaces are cut FROM, and
// nothing opens it — so this page is about the project itself. The one thing it
// asks its machine for is the project's conversation owner, which the bridge
// mints in a scratch directory of its own.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const collapseChat = vi.fn();
const mountAgentRail = vi.fn(() => ({ collapse: collapseChat, dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));

const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));
const openProjectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: (...args) => openProjectSettings(...args) }));

// The Tasks tab is its own surface with its own reads and its own push; this
// file is about the page that holds it, so it is mocked to the handle the page
// keeps.
const readProjectFilesSupport = vi.fn(async () => false);
vi.mock("../src/core/projectFilesSupport.js", async (importOriginal) => ({
  ...await importOriginal(), readProjectFilesSupport: (...args) => readProjectFilesSupport(...args),
}));

const renderFilesTab = vi.fn(() => ({ dispose: vi.fn(), canLeave: vi.fn(async () => true), hasUnsavedChanges: () => false }));
vi.mock("../src/views/files.js", () => ({ renderFilesTab: (...args) => renderFilesTab(...args) }));

const mountTasksPane = vi.fn(() => ({ feedMoved: vi.fn(), dispose: vi.fn() }));
vi.mock("../src/core/trackerTasksPane.js", () => ({ mountTasksPane: (...args) => mountTasksPane(...args) }));

let subscribers = [];
let snapshot = { items: [], projects: [], workspaces: [], pending: [], devices: {} };
const refreshFeed = vi.fn(async () => []);
const deliver = () => subscribers.forEach((fn) => fn(snapshot));
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

import { App } from "../src/app.js";
import { renderProject } from "../src/views/projectView.js";
import { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { standShell, stopShell } from "../src/core/shell.js";
import { fakeSession } from "./deviceSessionFixture.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Open the page the way the app opens it. The rail beside it is the SHELL's
 *  now (core/shell.js) — `render()` stands it on the route before the page
 *  paints — so a case that goes straight to the view would be testing a page
 *  in a shell that was never stood. */
const openProject = async () => {
  standShell(App.route);
  await renderProject();
};

/** The Workspaces tab, which a project URL no longer opens on its own (#46). */
const openWorkspacesTab = async () => {
  App.route = { ...App.route, tab: "workspaces" };
  await openProject();
};

const workspace = (id, extra = {}) => ({
  id,
  workspace_id: id,
  project_id: "proj-1",
  deviceId: "dev-1",
  projectKey: "dev-1/proj-1",
  workspaceKey: `dev-1/${id}`,
  status: "ready",
  name: id,
  updated_at: "2026-01-01T00:00:00Z",
  directories: [{ source_id: "repo", branch: "build/login", is_git: true }],
  ...extra,
});

const project = { id: "proj-1", project_id: "proj-1", name: "Build", deviceId: "dev-1", projectKey: "dev-1/proj-1" };

const device = (deviceId, answer = async () => ({})) => {
  const call = vi.fn(answer);
  // Greeted, too: a project with no owner is minted one only once its
  // bridge has said which API it speaks (core/shell.js).
  adoptBridgeSelection(adoptDeviceSession({ ...fakeSession(deviceId), call }), { version: "2.0.0" }, null);
  return call;
};

let here;
let elsewhere;
const widthIs = (px) => Object.defineProperty(window, "innerWidth", { configurable: true, value: px });

beforeEach(() => {
  localStorage.clear();
  widthIs(1024); // jsdom's own, which a case that sets a phone or a desktop must not leave behind
  document.body.innerHTML =
    '<div id="toolbar"><span id="tb-verb"></span></div><nav id="dir-rail"></nav><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  mountAgentRail.mockClear();
  collapseChat.mockClear();
  mountTasksPane.mockClear();
  renderFilesTab.mockClear();
  readProjectFilesSupport.mockReset().mockResolvedValue(false);
  openCreateWork.mockClear();
  openProjectSettings.mockClear();
  subscribers = [];
  snapshot = { items: [], projects: [project], workspaces: [workspace("ws-1"), workspace("ws-2", { name: "docs" })], pending: [], devices: {} };
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
  App.devices = [
    { id: "dev-1", name: "this machine", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1" };
  location.hash = "#/device/dev-1/project/proj-1";
  here = device("dev-1", async (method) =>
    method === "project.ensure_conversation" ? { project_id: "proj-1", entity_id: "run-7", run_id: "run-7" } : {},
  );
  elsewhere = device("dev-2");
});

afterEach(() => {
  expect(elsewhere).not.toHaveBeenCalled();
  App.viewDispose?.();
  App.viewDispose = null;
  stopShell();
  resetDeviceContexts();
});

const rows = () => [...document.querySelectorAll("[data-workspace]")];
const rail = () => document.querySelector("#dir-rail");
const pressProjectTab = (tab) => rail().querySelector(`[data-tab="${tab}"]`).click();

describe("the project surface", () => {
  it("lists the project's workspaces, each opening its own surface", async () => {
    await openWorkspacesTab();
    await flush();

    expect(rows().map((row) => row.querySelector(".stitle").textContent)).toEqual(["ws-1", "docs"]);
    expect(rows()[0].textContent).toContain("build/login");
    rows()[0].click();
    expect(location.hash).toBe("#/device/dev-1/project/proj-1/workspace/ws-1/directory/repo/changes");
  });

  it("repaints when the feed moves", async () => {
    await openWorkspacesTab();
    await flush();
    expect(rows()).toHaveLength(2);

    snapshot = { ...snapshot, workspaces: [workspace("ws-1")] };
    deliver();

    expect(rows()).toHaveLength(1);
  });

  // A project with nothing in it is the ordinary first state of a project, and
  // the page says what to do about it rather than looking broken.
  it("says there are no workspaces yet and points at the +", async () => {
    snapshot = { ...snapshot, workspaces: [] };
    await openWorkspacesTab();
    await flush();

    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#root").textContent).toMatch(/no workspaces/i);
    expect(document.querySelector("#root").textContent).toContain("+");
  });

  // The rail is the project's own agent. Its owner is minted by the bridge in a
  // scratch directory — never in the project's checkout — and the page asks for
  // it exactly the way a workspace asks for its own.
  it("mounts the rail on the owner project.ensure_conversation answers with", async () => {
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));

    expect(here).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" });
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    const [host, options] = mountAgentRail.mock.calls[0];
    expect(host.id).toBe("agent-rail");
    expect(options.kind).toBe("project");
    expect(options.entityId).toBe("run-7");
    expect(options.projectId).toBe("proj-1");
    expect(options.deviceId).toBe("dev-1");
  });

  // A link from a message to the conversation it came from names the agent on
  // the route: the page opens with the rail standing on that conversation.
  it("opens the rail on the agent the route names", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", agent: "ag-2" };
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));

    expect(mountAgentRail.mock.calls[0][1].openAgentId).toBe("ag-2");
  });

  it("names no agent where the route names none", async () => {
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));

    expect(mountAgentRail.mock.calls[0][1].openAgentId).toBe(null);
  });

  // What a project agent starts on is the DEVICE's setting, so the page names
  // none of it: the bridge mints the owner on its own answer. A browser slot
  // laid over it would ask every new browser for something the machine that
  // runs the agent already holds.
  it("names no harness, model or effort — the device's setting decides", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "codex", harnesses: { codex: { model: "gpt-5.6-sol", effort: "medium" } },
    }));
    await openProject();
    await vi.waitFor(() => expect(here).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" }));

    const ensured = here.mock.calls.filter(([method]) => method === "project.ensure_conversation");
    expect(ensured).toHaveLength(1);
    expect(ensured[0][1]).toEqual({ project_id: "proj-1" });
  });

  // The verb slot is where this page's verb lives, and it says which project
  // it is about: the bar names one project, and so does it. The settings are
  // the rail's now (#274).
  it("offers the project's + on the toolbar, named for it, and no cog", async () => {
    await openProject();
    await flush();

    const create = document.querySelector("[data-project-create]");
    expect(create.getAttribute("aria-label")).toBe("New workspace in Build");
    expect(document.querySelector("#tb-verb [data-project-settings]")).toBeNull();

    create.click();
    expect(openCreateWork).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: "proj-1", deviceId: "dev-1", projectName: "Build" }),
    );
  });

  it("hands the verb slot back when the surface goes", async () => {
    await openProject();
    await flush();
    expect(document.querySelector("[data-project-create]")).toBeTruthy();

    App.viewDispose();
    App.viewDispose = null;

    expect(document.querySelector("[data-project-create]")).toBeNull();
  });

  // A machine that cannot answer has nothing under this link: the surface names
  // it rather than standing a frame up over calls that can only be refused.
  it("names the machine instead when it cannot answer", async () => {
    App.route = { name: "project", deviceId: "dev-9", projectId: "proj-1" };
    await openProject();
    await flush();

    expect(mountAgentRail).not.toHaveBeenCalled();
    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#root").textContent).toBeTruthy();
  });
});


// A project holds two kinds of thing: the workspaces the work happens in, and
// the tasks that say what the work IS.
// The two tabs are the faces of the project's rail (#274); a press on one
// switches the page in place.
describe("the project's two tabs", () => {
  // #46: a bare project link opens the tracker now, and Workspaces is the tab
  // that names itself.
  it("opens on the tasks, which is what a project URL opens on now", async () => {
    await openProject();
    await flush();
    expect(mountTasksPane).toHaveBeenCalled();
    expect(document.querySelector(".project-rows")).toBeNull();
  });

  it("opens the workspaces when the route names that tab", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "workspaces" };
    await openProject();
    await flush();
    expect(document.querySelector(".project-rows")).not.toBeNull();
    expect(mountTasksPane).not.toHaveBeenCalled();
  });

  it("mounts the Tasks tab on the machine the project is on", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks" };
    await openProject();
    await flush();
    const [host, given] = mountTasksPane.mock.calls[0];
    expect(host.id).toBe("project-pane");
    expect([given.projectId, given.deviceId, given.projectKey]).toEqual(["proj-1", "dev-1", "dev-1/proj-1"]);
  });

  // Each tab owns the body outright, so the one leaving is torn down before
  // the one arriving is built.
  it("tears the Tasks tab down on the way back to the workspaces", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks" };
    await openProject();
    await flush();
    const pane = mountTasksPane.mock.results[0].value;
    pressProjectTab("workspaces");
    await flush();
    expect(pane.dispose).toHaveBeenCalled();
    expect(document.querySelector(".project-rows")).not.toBeNull();
  });

  // The page is the same page: a navigation would remount the rail beside it.
  it("rewrites the hash rather than navigating", async () => {
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));
    mountAgentRail.mockClear();
    pressProjectTab("workspaces");
    await flush();
    expect(location.hash).toBe("#/device/dev-1/project/proj-1/workspaces");
    expect(mountAgentRail).not.toHaveBeenCalled();
  });

  it("keeps a board link a board link", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks", view: "board" };
    await openProject();
    await flush();
    expect(mountTasksPane.mock.calls[0][1].view).toBe("board");
  });

  it("writes the view the tab moved to into the hash", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks" };
    await openProject();
    await flush();
    mountTasksPane.mock.calls[0][1].onViewChange("board");
    expect(location.hash).toBe("#/device/dev-1/project/proj-1?view=board");
  });

  it("takes the Tasks tab down with the page", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "tasks" };
    await openProject();
    await flush();
    const pane = mountTasksPane.mock.results[0].value;
    App.viewDispose();
    App.viewDispose = null;
    expect(pane.dispose).toHaveBeenCalled();
  });
});

// #274: "The project view should use a left rail the way a workspace does,
// with three entries only: Workspaces, Tasks and the project's Settings."
describe("the project's rail", () => {
  const faces = () => [...rail().querySelectorAll("[data-tab]")].map((cell) => [cell.dataset.tab, cell.getAttribute("aria-selected")]);

  it("stands Tasks, Workspaces and Settings on the shell's rail, and nothing else", async () => {
    await openProject();
    await flush();
    expect(faces()).toEqual([["tasks", "true"], ["files", "false"], ["workspaces", "false"]]);
    expect(rail().querySelectorAll("button")).toHaveLength(4);
    expect(rail().querySelector("[data-rail-settings]").getAttribute("aria-label")).toBe("Project settings");
    expect(rail().querySelector("[data-tab=changes], [data-sidebar-toggle]")).toBeNull();
  });

  it("marks the face the route stands on, and follows a press", async () => {
    App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "workspaces" };
    await openProject();
    await flush();
    expect(faces()).toEqual([["tasks", "false"], ["files", "false"], ["workspaces", "true"]]);
    pressProjectTab("tasks");
    await flush();
    expect(faces()).toEqual([["tasks", "true"], ["files", "false"], ["workspaces", "false"]]);
    expect(location.hash).toBe("#/device/dev-1/project/proj-1");
  });

  it("switches the tab without remounting the page", async () => {
    await openProject();
    await flush();
    const root = document.querySelector("#root");
    const pane = document.querySelector("#project-pane");
    pressProjectTab("workspaces");
    await flush();
    expect(document.querySelector("#project-pane")).toBe(pane);
    expect(document.querySelector("#root")).toBe(root);
    expect(document.querySelector(".project-rows")).not.toBeNull();
  });

  // A press switches the tab in place, so the shell's route rule never sees a
  // navigation (#62): where the chat lies over the page, the press puts it away
  // itself, and beside the page it leaves it be.
  it("puts the chat away on a press where it lies over the page", async () => {
    widthIs(390);
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));
    pressProjectTab("workspaces");
    expect(collapseChat).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".project-rows")).not.toBeNull();
  });

  it("leaves the chat open on a press where it stands beside the page", async () => {
    widthIs(1400);
    await openProject();
    await vi.waitFor(() => expect(mountAgentRail).toHaveBeenCalledTimes(1));
    pressProjectTab("workspaces");
    expect(collapseChat).not.toHaveBeenCalled();
  });

  it("opens the project's settings sheet from the cog at its foot", async () => {
    await openProject();
    await flush();
    rail().querySelector("[data-rail-settings]").click();
    expect(openProjectSettings).toHaveBeenCalledWith(
      "proj-1",
      expect.objectContaining({ callRpc: expect.any(Function), deviceId: "dev-1" }),
    );
  });

  it("draws no project tabs in the bar", async () => {
    await openProject();
    await flush();
    expect(document.querySelector("#toolbar [data-project-tab]")).toBeNull();
  });

  it("hands the shell's column back empty when the page goes", async () => {
    await openProject();
    await flush();
    App.viewDispose();
    App.viewDispose = null;
    expect(rail().children).toHaveLength(0);
  });
});

// The reclaim service's verdict on each workspace (#135), read off the cached
// rows: a line saying what holds a quiet workspace, and a Reclaim when nothing
// does.
describe("a workspace's lifecycle on the Workspaces tab", () => {
  const verdict = (extra = {}) => ({
    idle: true, reclaimable: false, holds: [], tasks: [], dirty_files: 0, unpushed_commits: 0,
    behind_commits: 0, size_bytes: 17_200_000_000, pruned_bytes: 0, pruned_at_ms: null, noticed_at_ms: null,
    measured_at_ms: 1, last_activity_ms: 0, ...extra,
  });
  const reclaimButton = (row) => row.querySelector("[data-workspace-reclaim]");

  beforeEach(() => {
    snapshot = {
      ...snapshot,
      workspaces: [
        workspace("ws-1", { lifecycle: verdict({ reclaimable: true }) }),
        workspace("ws-2", { name: "docs", lifecycle: verdict({ holds: ["dirty"], dirty_files: 3 }) }),
      ],
    };
  });

  it("says what holds a quiet workspace and offers Reclaim only where nothing does", async () => {
    await openWorkspacesTab();
    await flush();

    expect(rows()[0].textContent).toContain("Reclaimable · 17.2 GB");
    expect(reclaimButton(rows()[0])).not.toBeNull();
    expect(rows()[1].textContent).toContain("Idle · 3 uncommitted files · 17.2 GB");
    expect(reclaimButton(rows()[1])).toBeNull();
  });

  it("reclaims through the bridge without opening the workspace", async () => {
    await openWorkspacesTab();
    await flush();
    const before = location.hash;

    reclaimButton(rows()[0]).click();
    await vi.waitFor(() => expect(refreshFeed).toHaveBeenCalledWith("dev-1"));

    expect(here).toHaveBeenCalledWith("workspace.reclaim", { workspace_id: "ws-1" });
    expect(location.hash).toBe(before);
  });

  it("shows the bridge's refusal on the row and offers Reclaim again", async () => {
    here.mockImplementation(async (method) => {
      if (method === "workspace.reclaim") throw new Error("Build cannot reclaim ws-1 yet: it has uncommitted changes.");
      return {};
    });
    await openWorkspacesTab();
    await flush();

    reclaimButton(rows()[0]).click();
    await vi.waitFor(() =>
      expect(rows()[0].querySelector("[data-reclaim-error]").textContent)
        .toBe("Build cannot reclaim ws-1 yet: it has uncommitted changes."));
    expect(reclaimButton(rows()[0]).disabled).toBe(false);
  });
});

// #167: a filter down to what can be reclaimed, biggest first, and a size on
// every row, all from the cached workspace.list rows.
describe("the Reclaimable filter and the size column", () => {
  const verdict = (reclaimable, size) => ({
    idle: true, reclaimable, holds: reclaimable ? [] : ["dirty"], tasks: [], dirty_files: 1,
    unpushed_commits: 0, behind_commits: 0, size_bytes: size, pruned_bytes: 0, pruned_at_ms: null,
    noticed_at_ms: null, measured_at_ms: 1, last_activity_ms: 0,
  });
  const filter = (name) => document.querySelector(`[data-workspace-filter="${name}"]`);
  const shownIds = () => rows().map((row) => row.dataset.workspace);
  const sizes = () => rows().map((row) => row.querySelector(".project-size").textContent);

  beforeEach(() => {
    snapshot = {
      ...snapshot,
      workspaces: [
        workspace("small", { lifecycle: verdict(true, 2_000_000_000) }),
        workspace("held", { lifecycle: verdict(false, 40_000_000_000) }),
        workspace("large", { lifecycle: verdict(true, 17_200_000_000) }),
        workspace("fresh"),
      ],
    };
  });

  it("lists every workspace in its order, each with its size, until narrowed", async () => {
    await openWorkspacesTab();
    await flush();

    expect(shownIds()).toEqual(["dev-1/small", "dev-1/held", "dev-1/large", "dev-1/fresh"]);
    expect(sizes()).toEqual(["2.0 GB", "40.0 GB", "17.2 GB", ""]);
    expect(filter("all").getAttribute("aria-pressed")).toBe("true");
    expect(filter("reclaimable").textContent).toBe("Reclaimable (2)");
  });

  it("narrows to the reclaimable workspaces, largest first, and back", async () => {
    await openWorkspacesTab();
    await flush();
    const before = location.hash;

    filter("reclaimable").click();

    expect(shownIds()).toEqual(["dev-1/large", "dev-1/small"]);
    expect(sizes()).toEqual(["17.2 GB", "2.0 GB"]);
    expect(filter("reclaimable").getAttribute("aria-pressed")).toBe("true");
    expect(location.hash).toBe(before);

    filter("all").click();
    expect(shownIds()).toHaveLength(4);
  });

  it("keeps the filter when the feed moves", async () => {
    await openWorkspacesTab();
    await flush();
    filter("reclaimable").click();

    snapshot = { ...snapshot, workspaces: [...snapshot.workspaces, workspace("huge", { lifecycle: verdict(true, 90_000_000_000) })] };
    deliver();

    expect(shownIds()).toEqual(["dev-1/huge", "dev-1/large", "dev-1/small"]);
  });

  // Review 1 (P2): the press repainted the pane and took the focused button
  // with it, so focus fell to the body.
  it("keeps keyboard focus on the filter pressed", async () => {
    await openWorkspacesTab();
    await flush();
    filter("reclaimable").focus();

    filter("reclaimable").click();

    expect(document.activeElement).toBe(filter("reclaimable"));
    expect(document.activeElement.getAttribute("aria-pressed")).toBe("true");
    expect(shownIds()).toEqual(["dev-1/large", "dev-1/small"]);

    filter("all").focus();
    filter("all").click();
    expect(document.activeElement).toBe(filter("all"));
  });

  it("keeps keyboard focus on a filter when the feed moves", async () => {
    await openWorkspacesTab();
    await flush();
    filter("reclaimable").focus();

    snapshot = { ...snapshot, workspaces: [...snapshot.workspaces, workspace("huge", { lifecycle: verdict(true, 90_000_000_000) })] };
    deliver();

    expect(document.activeElement).toBe(filter("reclaimable"));
    expect(filter("reclaimable").textContent).toBe("Reclaimable (3)");
  });

  it("keeps keyboard focus on a row's Reclaim when the feed moves", async () => {
    await openWorkspacesTab();
    await flush();
    const reclaim = () => document.querySelector('[data-workspace-reclaim="dev-1/large"]');
    reclaim().focus();

    snapshot = { ...snapshot, workspaces: [...snapshot.workspaces, workspace("huge", { lifecycle: verdict(true, 90_000_000_000) })] };
    deliver();

    expect(document.activeElement).toBe(reclaim());
  });

  it("says so when nothing can be reclaimed", async () => {
    snapshot = { ...snapshot, workspaces: [workspace("held", { lifecycle: verdict(false, 1_000) })] };
    await openWorkspacesTab();
    await flush();

    filter("reclaimable").click();

    expect(rows()).toEqual([]);
    expect(document.querySelector(".project-filter-empty").textContent)
      .toBe("No workspace can be reclaimed right now.");
  });
});


describe("project Files", () => {
  it("does not overwrite a newer surface when the cold support read finishes", async () => {
    let finishRead;
    readProjectFilesSupport.mockImplementationOnce(() => new Promise((resolve) => { finishRead = resolve; }));
    const pending = renderProject();
    const nextDispose = vi.fn();
    App.route = { name: "inbox" };
    App.viewDispose = nextDispose;
    document.querySelector("#root").innerHTML = '<div class="newer-surface">Inbox</div>';
    finishRead(false);
    await pending;
    expect(document.querySelector(".newer-surface").textContent).toBe("Inbox");
    expect(App.viewDispose).toBe(nextDispose);
    expect(renderFilesTab).not.toHaveBeenCalled();
  });
  it("keeps dirty drafts and warns when a source folder moves, and remounts clean folders", async () => {
    snapshot = { ...snapshot, projects: [{ ...project, sources: [{ id: "docs", name: "Docs", path: "/old" }] }] };
    App.route = { ...App.route, tab: "files" };
    await openProject();
    const explorer = renderFilesTab.mock.results[0].value;
    explorer.hasUnsavedChanges = () => true;
    snapshot = { ...snapshot, projects: [{ ...project, sources: [{ id: "docs", name: "Docs", path: "/new" }] }] };
    deliver();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    expect(explorer.dispose).not.toHaveBeenCalled();
    expect(document.querySelector("[data-project-files-support]").textContent).toContain("folder moved");
    expect(document.querySelector("[data-project-files-support]").hidden).toBe(false);
    explorer.hasUnsavedChanges = () => false;
    deliver();
    expect(explorer.dispose).toHaveBeenCalledTimes(1);
    expect(renderFilesTab).toHaveBeenCalledTimes(2);
    expect(renderFilesTab.mock.calls[1][1].roots[0].cacheEntityId).toBe('project:["proj-1","docs","/new"]');
  });
  beforeEach(() => {
    snapshot = { ...snapshot, projects: [{ ...project, sources: [{ id: "code", name: "Code" }, { id: "docs", name: "Docs" }] }] };
  });

  it("mounts the shared explorer on project source roots and restores its deep link", async () => {
    App.route = { ...App.route, tab: "files", sourceId: "docs", file: "README.md", line: 7, agent: "a1" };
    await openProject();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    const options = renderFilesTab.mock.calls[0][1];
    expect(options.roots.map((root) => root.scope)).toEqual([{ project_id: "proj-1", source_id: "code" }, { project_id: "proj-1", source_id: "docs" }]);
    expect(options.openAt).toEqual({ rootId: "docs", path: "README.md", line: 7 });
    options.onFileOpen("a.js", "code");
    expect(location.hash).toBe("#/device/dev-1/project/proj-1/files?agent=a1&source=code&path=a.js");
    pressProjectTab("tasks");
    await flush();
    pressProjectTab("files");
    await flush();
    expect(renderFilesTab.mock.calls[1][1].openAt).toEqual({ rootId: "code", path: "a.js", line: null });
  });

  it("keeps the explorer and its unsaved editor when unrelated feed records repaint", async () => {
    App.route = { ...App.route, tab: "files" };
    await openProject();
    const explorer = renderFilesTab.mock.results[0].value;
    snapshot = { ...snapshot, workspaces: [workspace("another")] };
    deliver();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    expect(explorer.dispose).not.toHaveBeenCalled();
  });

  it("honors Files' leave guard on tab presses and clears its route guard on teardown", async () => {
    App.route = { ...App.route, tab: "files" };
    await openProject();
    const explorer = renderFilesTab.mock.results[0].value;
    explorer.canLeave.mockResolvedValue(false);
    expect(App.routeLeaveGuard).toBe(explorer.canLeave);
    pressProjectTab("workspaces");
    await flush();
    expect(explorer.dispose).not.toHaveBeenCalled();
    expect(App.route.tab).toBe("files");
    explorer.canLeave.mockResolvedValue(true);
    pressProjectTab("workspaces");
    await flush();
    expect(explorer.dispose).toHaveBeenCalledTimes(1);
    expect(App.routeLeaveGuard).toBeNull();
  });
});
