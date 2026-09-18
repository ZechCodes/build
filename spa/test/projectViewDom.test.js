/** @vitest-environment jsdom */
// The project's own surface: what the project holds in the main pane, and who
// you talk to about the project in the rail.
//
// A project is a template — its checkout is what workspaces are cut FROM, and
// nothing opens it — so this page is about the project itself. The one thing it
// asks its machine for is the project's conversation owner, which the bridge
// mints in a scratch directory of its own.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mountAgentRail = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));

const openCreateWork = vi.fn();
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: (...args) => openCreateWork(...args) }));
const openProjectSettings = vi.fn();
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: (...args) => openProjectSettings(...args) }));

let subscribers = [];
let snapshot = { items: [], projects: [], workspaces: [], pending: [], devices: {} };
const refreshFeed = vi.fn(async () => {});
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
import { adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

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
  adoptDeviceSession({ ...fakeSession(deviceId), call });
  return call;
};

let here;
let elsewhere;

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML =
    '<div id="toolbar"><span id="tb-verb"></span></div><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  mountAgentRail.mockClear();
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
  resetDeviceContexts();
});

const rows = () => [...document.querySelectorAll("[data-workspace]")];

describe("the project surface", () => {
  it("lists the project's workspaces, each opening its own surface", async () => {
    await renderProject();
    await flush();

    expect(rows().map((row) => row.querySelector(".stitle").textContent)).toEqual(["ws-1", "docs"]);
    expect(rows()[0].textContent).toContain("build/login");
    rows()[0].click();
    expect(location.hash).toBe("#/device/dev-1/project/proj-1/workspace/ws-1/directory/repo/changes");
  });

  it("repaints when the feed moves", async () => {
    await renderProject();
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
    await renderProject();
    await flush();

    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#root").textContent).toMatch(/no workspaces/i);
    expect(document.querySelector("#root").textContent).toContain("+");
  });

  // The rail is the project's own agent. Its owner is minted by the bridge in a
  // scratch directory — never in the project's checkout — and the page asks for
  // it exactly the way a workspace asks for its own.
  it("mounts the rail on the owner project.ensure_conversation answers with", async () => {
    await renderProject();
    await flush();

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
    await renderProject();
    await flush();

    expect(mountAgentRail.mock.calls[0][1].openAgentId).toBe("ag-2");
  });

  it("names no agent where the route names none", async () => {
    await renderProject();
    await flush();

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
    await renderProject();
    await flush();

    const ensured = here.mock.calls.filter(([method]) => method === "project.ensure_conversation");
    expect(ensured).toHaveLength(1);
    expect(ensured[0][1]).toEqual({ project_id: "proj-1" });
  });

  // The verb slot is where this page's two verbs live, and both say which
  // project they are about: the bar names one project, and so do they.
  it("offers the project's own verbs on the toolbar, named for it", async () => {
    await renderProject();
    await flush();

    const create = document.querySelector("[data-project-create]");
    const settings = document.querySelector("[data-project-settings]");
    expect(create.getAttribute("aria-label")).toBe("New workspace in Build");
    expect(settings.getAttribute("aria-label")).toBe("Settings for Build");

    create.click();
    expect(openCreateWork).toHaveBeenCalledWith(
      expect.objectContaining({ projectId: "proj-1", deviceId: "dev-1", projectName: "Build" }),
    );
    settings.click();
    expect(openProjectSettings).toHaveBeenCalledWith("proj-1", expect.objectContaining({ callRpc: expect.any(Function) }));
  });

  it("hands the verb slot back when the surface goes", async () => {
    await renderProject();
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
    await renderProject();
    await flush();

    expect(mountAgentRail).not.toHaveBeenCalled();
    expect(rows()).toHaveLength(0);
    expect(document.querySelector("#root").textContent).toBeTruthy();
  });
});
