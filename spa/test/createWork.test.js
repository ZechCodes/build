// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { motionBeat } from "./motionRecorder.js";

const { App } = await import("../src/app.js");
const { CREATE_KINDS, createWorkHtml, openCreateWork, workspaceCreateParams } = await import("../src/core/createWork.js");
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const modal = () => document.querySelector("#create-scrim .modal");
let navigate;

beforeEach(() => {
  document.body.innerHTML = "";
  navigate = vi.fn();
  App.call = vi.fn(async () => ({
    id: "ws-1", project_id: "p1", name: "Release", status: "ready",
    directories: [{ source_id: "frontend", is_git: true }, { source_id: "assets", is_git: false }],
  }));
});

describe("workspace creation", () => {
  it("is the only active creation kind and omits inherited optional values", () => {
    expect(CREATE_KINDS).toEqual(["workspace"]);
    expect(workspaceCreateParams({ projectId: "p1", name: "  ", isolation: "" })).toEqual({ project_id: "p1" });
  });

  it("creates all project sources and opens the returned workspace", async () => {
    openCreateWork({ projectId: "p1", projectName: "Payments", navigate });
    modal().querySelector("#create-work-input").value = " Release ";
    modal().querySelector("#create-work-input").dispatchEvent(new Event("input"));
    modal().querySelector("#create-work-isolation").value = "rift";
    modal().querySelector("#create-work-isolation").dispatchEvent(new Event("change"));
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("workspace.create", { project_id: "p1", name: "Release", isolation: "rift" });
    expect(navigate).toHaveBeenCalledWith({
      name: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "frontend", tab: "changes",
    });
    expect(modal()).toBeNull();
  });

  it("keeps the draft and permits retry after a create failure", async () => {
    App.call = vi.fn().mockRejectedValueOnce(new Error("another filesystem operation is still running")).mockResolvedValueOnce({
      id: "ws-1", project_id: "p1", directories: [{ source_id: "frontend", is_git: true }],
    });
    openCreateWork({ projectId: "p1", projectName: "Payments", navigate });
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
    expect(App.call).toHaveBeenCalledTimes(2);
  });

  it("deduplicates presses while creation is pending", async () => {
    let complete;
    App.call = vi.fn(() => new Promise((resolve) => { complete = resolve; }));
    openCreateWork({ projectId: "p1", projectName: "Payments", navigate });
    const button = modal().querySelector("[data-create-go]");
    button.click();
    button.click();
    expect(App.call).toHaveBeenCalledTimes(1);
    complete({ id: "ws-1", project_id: "p1", directories: [] });
    await flush();
  });

  it("blocks creation without a connected project and closes on Cancel", async () => {
    App.call = null;
    openCreateWork({ projectId: null, projectName: "Project", navigate });
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
