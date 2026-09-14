// @vitest-environment jsdom
// The one create surface: a modal that makes a workspace, opened by the
// toolbar's project menu and by the rail's project blocks alike. A project
// belongs to one machine, so the dialog is opened with that machine and
// everything it asks for goes there.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { motionBeat } from "./motionRecorder.js";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

const refreshFeed = vi.fn(async () => {});
vi.mock("../src/core/taskFeed.js", () => ({
  refreshFeed: (...args) => refreshFeed(...args),
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  primaryRunIdFor: () => null,
  dropFeedDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { CREATE_KINDS, createWorkHtml, openCreateWork, workspaceCreateParams } = await import("../src/core/createWork.js");

/** A device on the account, answering with `call`. */
const deviceAnswering = (deviceId, call) =>
  adoptDeviceSession({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const modal = () => document.querySelector("#create-scrim .modal");
let navigate;

const CREATED = {
  id: "ws-1",
  project_id: "p1",
  name: "Release",
  status: "ready",
  directories: [{ source_id: "frontend", is_git: true }, { source_id: "assets", is_git: false }],
};

beforeEach(() => {
  document.body.innerHTML = "";
  resetDeviceContexts();
  navigate = vi.fn();
  App.devices = [{ id: "dev-1", name: "Laptop", status: "online" }];
  App.selectedDeviceId = "dev-1";
  bridge.call = vi.fn(async () => CREATED);
  deviceAnswering("dev-1", (...args) => bridge.call(...args));
});

describe("workspace creation", () => {
  it("is the only active creation kind and omits inherited optional values", () => {
    expect(CREATE_KINDS).toEqual(["workspace"]);
    expect(workspaceCreateParams({ projectId: "p1", name: "  ", isolation: "" })).toEqual({ project_id: "p1" });
  });

  it("creates all project sources and opens the returned workspace on its device", async () => {
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    modal().querySelector("#create-work-input").value = " Release ";
    modal().querySelector("#create-work-input").dispatchEvent(new Event("input"));
    modal().querySelector("#create-work-isolation").value = "rift";
    modal().querySelector("#create-work-isolation").dispatchEvent(new Event("change"));
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("workspace.create", { project_id: "p1", name: "Release", isolation: "rift" });
    expect(navigate).toHaveBeenCalledWith({
      name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes",
    });
    expect(modal()).toBeNull();
  });

  it("calls the scoped device's bridge, not another machine's", async () => {
    const other = vi.fn(async () => CREATED);
    deviceAnswering("dev-2", other);
    App.devices = [...App.devices, { id: "dev-2", name: "Studio", status: "online" }];

    openCreateWork({ projectId: "p1", deviceId: "dev-2", projectName: "Payments", navigate });
    modal().querySelector("[data-create-go]").click();
    await flush();

    expect(other).toHaveBeenCalledWith("workspace.create", { project_id: "p1" });
    expect(bridge.call).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ deviceId: "dev-2" }));
  });

  it("keeps the draft and permits retry after a create failure", async () => {
    bridge.call = vi.fn()
      .mockRejectedValueOnce(new Error("another filesystem operation is still running"))
      .mockResolvedValueOnce({ id: "ws-1", project_id: "p1", directories: [{ source_id: "frontend", is_git: true }] });
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    const input = modal().querySelector("#create-work-input");
    input.value = "Release";
    input.dispatchEvent(new Event("input"));
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toContain("filesystem operation");
    expect(modal().querySelector("#create-work-input").value).toBe("Release");
    expect(modal().querySelector("[data-create-go]").disabled).toBe(false);
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(bridge.call).toHaveBeenCalledTimes(2);
  });

  it("deduplicates presses while creation is pending", async () => {
    let complete;
    bridge.call = vi.fn(() => new Promise((resolve) => { complete = resolve; }));
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    const button = modal().querySelector("[data-create-go]");
    button.click();
    button.click();
    expect(bridge.call).toHaveBeenCalledTimes(1);
    complete({ id: "ws-1", project_id: "p1", directories: [] });
    await flush();
  });

  // No context for the device is a machine this client cannot ask anything:
  // the dialog says so rather than sending a call that can only be refused.
  it("blocks creation without a connected project and closes on Cancel", async () => {
    openCreateWork({ projectId: null, deviceId: "dev-9", projectName: "Project", navigate });
    modal().querySelector("[data-create-go]").click();
    expect(modal().querySelector(".create-error").textContent).toContain("connected project");
    modal().querySelector("[data-create-cancel]").click();
    await motionBeat();
    expect(modal()).toBeNull();
  });

  it("escapes project names", () => {
    expect(createWorkHtml({ projectName: "<b>x</b>", name: "", isolation: "", busy: false, error: "" })).toContain("&lt;b&gt;x&lt;/b&gt;");
  });
});
