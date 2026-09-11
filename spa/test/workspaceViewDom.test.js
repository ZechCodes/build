/** @vitest-environment jsdom */
import { beforeEach, describe, expect, it, vi } from "vitest";

const { mountGitPane, mountConsole, mountAgentRail, renderFilesTab } = vi.hoisted(() => ({
  mountGitPane: vi.fn((host) => {
    host.innerHTML = '<aside class="crail-host"></aside>';
    return { dispose: vi.fn() };
  }),
  mountConsole: vi.fn(() => ({ dispose: vi.fn() })),
  mountAgentRail: vi.fn(() => ({ dispose: vi.fn() })),
  renderFilesTab: vi.fn(() => ({ dispose: vi.fn(), canLeave: vi.fn(async () => false) })),
}));

vi.mock("../src/core/gitPane.js", () => ({ mountGitPane }));
vi.mock("../src/core/console.js", () => ({ mountConsole }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail }));
vi.mock("../src/views/files.js", () => ({ renderFilesTab }));

import { App } from "../src/app.js";
import { renderWorkspace } from "../src/views/workspaceView.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const workspace = {
  id: "ws-1",
  project_id: "p-1",
  directories: [
    { source_id: "repo", name: "Repository", is_git: true },
    { source_id: "assets", name: "Assets", is_git: false },
  ],
};

beforeEach(() => {
  document.body.innerHTML = '<div id="toolbar"><span id="tb-verb"></span></div><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  mountGitPane.mockClear();
  mountConsole.mockClear();
  mountAgentRail.mockClear();
  renderFilesTab.mockClear();
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
});

describe("workspace surface", () => {
  it("scopes Files to a directory while terminals stay workspace scoped", async () => {
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    App.call = vi.fn(async () => workspace);
    await renderWorkspace();
    expect(renderFilesTab.mock.calls[0][1].scope).toEqual({ workspace_id: "ws-1", source_id: "assets" });
    expect(App.routeLeaveGuard).toBe(renderFilesTab.mock.results[0].value.canLeave);
    renderFilesTab.mock.calls[0][1].onFileOpen("logo.svg");
    expect(App.route.file).toBe("logo.svg");
    expect(mountConsole).toHaveBeenCalledWith(expect.anything(), { kind: "workspace", workspaceId: "ws-1" });
    expect(mountAgentRail).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({
      kind: "workspace", workspaceId: "ws-1", projectId: "p-1",
    }));
  });

  it("keeps the current ref and explains a checkout that would overwrite changes", async () => {
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    App.call = vi.fn(async (method, params) => {
      if (method === "workspace.get") return workspace;
      if (method === "git.refs") return {
        current: { kind: "branch", name: "main", full_ref: "refs/heads/main" },
        refs: [
          { kind: "branch", name: "main", full_ref: "refs/heads/main", current: true },
          { kind: "tag", name: "v1", full_ref: "refs/tags/v1", current: false },
        ],
      };
      if (method === "git.checkout_ref") throw new Error(`Cannot switch: local changes would be overwritten (${params.full_ref})`);
      return {};
    });
    await renderWorkspace();
    await flush();
    const select = document.querySelector("#workspace-ref");
    select.value = "refs/tags/v1";
    select.dispatchEvent(new Event("change"));
    await flush();
    expect(App.call).toHaveBeenCalledWith("git.checkout_ref", {
      workspace_id: "ws-1", source_id: "repo", full_ref: "refs/tags/v1",
    });
    expect(select.value).toBe("refs/heads/main");
    expect(document.querySelector(".workspace-referror").textContent).toContain("local changes would be overwritten");
  });

  it("restores Files and Changes tabs after a successful ref checkout remounts the Git pane", async () => {
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    App.call = vi.fn(async (method) => {
      if (method === "workspace.get") return workspace;
      if (method === "git.refs") return {
        current: { kind: "branch", name: "main", full_ref: "refs/heads/main" },
        refs: [
          { kind: "branch", name: "main", full_ref: "refs/heads/main", current: true },
          { kind: "tag", name: "v1", full_ref: "refs/tags/v1" },
        ],
      };
      return {};
    });
    await renderWorkspace();
    await flush();
    const select = document.querySelector("#workspace-ref");
    select.value = "refs/tags/v1";
    select.dispatchEvent(new Event("change"));
    await flush();
    await flush();
    expect(mountGitPane).toHaveBeenCalledTimes(2);
    expect([...document.querySelectorAll(".railtabs [data-tab]")].map((tab) => tab.dataset.tab)).toEqual(["changes", "files"]);
  });

  it("does not remount a Git pane after its source has been left", async () => {
    let finishCheckout;
    const checkout = new Promise((resolve) => { finishCheckout = resolve; });
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    App.call = vi.fn(async (method) => {
      if (method === "workspace.get") return workspace;
      if (method === "git.refs") return {
        current: { kind: "branch", name: "main", full_ref: "refs/heads/main" },
        refs: [{ kind: "branch", name: "main", full_ref: "refs/heads/main", current: true }, { kind: "tag", name: "v1", full_ref: "refs/tags/v1" }],
      };
      if (method === "git.checkout_ref") return checkout;
      return {};
    });
    await renderWorkspace();
    await flush();
    const select = document.querySelector("#workspace-ref");
    select.value = "refs/tags/v1";
    select.dispatchEvent(new Event("change"));
    App.viewDispose();
    finishCheckout({});
    await flush();
    expect(mountGitPane).toHaveBeenCalledTimes(1);
  });

  it("keeps the workspace and lists repositories that could not be pushed on Finish", async () => {
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    App.call = vi.fn(async (method) => method === "workspace.finish"
      ? { complete: false, repositories: [{ directory_id: "repo", pushed: false, reason: "no upstream" }] }
      : workspace);
    await renderWorkspace();
    document.querySelector("[data-workspace-action]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("workspace.finish", { workspace_id: "ws-1" });
    expect(document.querySelector(".workspace-action-status").textContent).toContain("repo: no upstream");
    expect(document.querySelector("#root").textContent).not.toContain("Workspace unavailable");
  });

  it("refreshes a failed workspace pane after Retry without replacing its terminal console", async () => {
    const failedWorkspace = { ...workspace, status: "failed" };
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    App.call = vi.fn(async (method) => method === "workspace.retry" ? { ...workspace, status: "ready" } : failedWorkspace);
    await renderWorkspace();
    const firstGuard = App.routeLeaveGuard;
    expect(document.querySelector("[data-workspace-action]").textContent).toBe("Retry");
    document.querySelector("[data-workspace-action]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("workspace.retry", { workspace_id: "ws-1" });
    expect(renderFilesTab).toHaveBeenCalledTimes(2);
    expect(App.routeLeaveGuard).not.toBe(firstGuard);
    expect(mountConsole).toHaveBeenCalledTimes(1);
  });

  it("preserves edits in an already-ready source while Retry repairs another source", async () => {
    const mixedWorkspace = {
      ...workspace,
      status: "failed",
      directories: [
        { source_id: "repo", name: "Repository", is_git: true, status: "failed" },
        { source_id: "assets", name: "Assets", is_git: false, status: "ready" },
      ],
    };
    const repaired = {
      ...mixedWorkspace,
      status: "ready",
      directories: mixedWorkspace.directories.map((directory) => ({ ...directory, status: "ready" })),
    };
    App.route = { name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    App.call = vi.fn(async (method) => method === "workspace.retry" ? repaired : mixedWorkspace);
    await renderWorkspace();
    const originalPane = renderFilesTab.mock.results[0].value;
    const originalGuard = App.routeLeaveGuard;
    document.querySelector("[data-workspace-action]").click();
    await flush();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    expect(originalPane.dispose).not.toHaveBeenCalled();
    expect(App.routeLeaveGuard).toBe(originalGuard);
    expect(mountConsole).toHaveBeenCalledTimes(1);
  });
});
