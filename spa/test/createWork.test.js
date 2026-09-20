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

const refreshFeed = vi.fn(async () => []);
vi.mock("../src/core/taskFeed.js", () => ({
  refreshFeed: (...args) => refreshFeed(...args),
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  dropFeedDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { adoptDeviceSession, resetDeviceContexts, setContextOffline } = await import(
  "../src/core/deviceContexts.js"
);
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
    expect(workspaceCreateParams({ projectId: "p1", name: "", isolation: "" })).toEqual({ project_id: "p1", name: "" });
  });

  it.each(["  ", "Bridge wire interface", "../feature: *?🦊", "a".repeat(500)])("sends the exact display name %j", (name) => {
    expect(workspaceCreateParams({ projectId: "p1", name })).toEqual({ project_id: "p1", name });
  });

  it("shows timed out creation in the modal instead of leaving it busy", async () => {
    bridge.call = vi.fn().mockRejectedValue(Object.assign(new Error("Request timed out"), { timedOut: true, uncertain: true }));
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toBe("Request timed out");
    expect(modal().querySelector("[data-create-go]").disabled).toBe(false);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("keeps a failed workspace reply in the error modal", async () => {
    bridge.call = vi.fn().mockResolvedValue({ ...CREATED, status: "failed", directories: [{ error: "Checkout failed" }] });
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toBe("Checkout failed");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("creates all project sources and opens the returned workspace on its device", async () => {
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    modal().querySelector("#create-work-input").value = " Release ";
    modal().querySelector("#create-work-input").dispatchEvent(new Event("input"));
    modal().querySelector("#create-work-isolation").value = "rift";
    modal().querySelector("#create-work-isolation").dispatchEvent(new Event("change"));
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("workspace.create", { project_id: "p1", name: " Release ", isolation: "rift" });
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

    expect(other).toHaveBeenCalledWith("workspace.create", { project_id: "p1", name: "" });
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
    expect(navigate).not.toHaveBeenCalled();
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

  it("freezes the submitted fields and keeps failures visible despite pending dismissal attempts", async () => {
    let fail;
    bridge.call = vi.fn(() => new Promise((resolve, reject) => { fail = reject; }));
    const dialog = openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    const input = modal().querySelector("#create-work-input");
    input.value = "  Bridge wire interface  ";
    input.dispatchEvent(new Event("input"));
    modal().querySelector("[data-create-go]").click();
    expect(modal().querySelector("#create-work-input").value).toBe("  Bridge wire interface  ");
    expect(modal().querySelector("#create-work-input").disabled).toBe(true);
    expect(modal().querySelector("#create-work-isolation").disabled).toBe(true);
    expect(modal().querySelector("[data-create-cancel]").disabled).toBe(true);
    modal().querySelector("[data-create-cancel]").click();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    document.querySelector("#create-scrim").click();
    dialog.close();
    await motionBeat();
    expect(modal()).not.toBeNull();

    fail(new Error("Checkout failed"));
    await flush();
    expect(modal().querySelector(".create-error").textContent).toBe("Checkout failed");
    expect(modal().querySelector("#create-work-input").value).toBe("  Bridge wire interface  ");
    expect(modal().querySelector("#create-work-input").disabled).toBe(false);
    expect(modal().querySelector("#create-work-isolation").disabled).toBe(false);
    expect(modal().querySelector("[data-create-cancel]").disabled).toBe(false);
    expect(navigate).not.toHaveBeenCalled();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await motionBeat();
    expect(modal()).toBeNull();
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

  // The dialog outlives an outage: a machine that was away when it opened is
  // asked the moment it is back, because the caller is read at the press and
  // never captured at the mount.
  it("creates on a machine that comes back while the dialog is open", async () => {
    setContextOffline("dev-1");
    openCreateWork({ projectId: "p1", deviceId: "dev-1", projectName: "Payments", navigate });
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(bridge.call).not.toHaveBeenCalled();
    expect(modal().querySelector(".create-error").textContent).toContain("connected project");

    setContextOffline("dev-1", { offline: false });
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(bridge.call).toHaveBeenCalledWith("workspace.create", { project_id: "p1", name: "" });
    expect(navigate).toHaveBeenCalled();
  });

  it("escapes project names", () => {
    expect(createWorkHtml({ projectName: "<b>x</b>", name: "", isolation: "", busy: false, error: "" })).toContain("&lt;b&gt;x&lt;/b&gt;");
  });
});
