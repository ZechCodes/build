/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { mountGitPane, mountConsole, mountAgentRail, renderFilesTab } = vi.hoisted(() => ({
  mountGitPane: vi.fn((host) => {
    host.innerHTML = '<aside class="crail-host"></aside>';
    return { dispose: vi.fn() };
  }),
  mountConsole: vi.fn(() => ({ dispose: vi.fn() })),
  mountAgentRail: vi.fn(() => ({ dispose: vi.fn() })),
  renderFilesTab: vi.fn((host) => {
    host.innerHTML = '<aside class="ftree"></aside><main class="file-editor"></main>';
    return { dispose: vi.fn(), canLeave: vi.fn(async () => false) };
  }),
}));

vi.mock("../src/core/gitPane.js", () => ({ mountGitPane }));
vi.mock("../src/core/console.js", () => ({ mountConsole }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail }));
vi.mock("../src/views/files.js", () => ({ renderFilesTab }));

import { App } from "../src/app.js";
import { SMALLEST_THREAD_PAGE } from "../src/core/thread.js";
import { renderWorkspace } from "../src/views/workspaceView.js";
import { adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const workspace = {
  id: "ws-1",
  project_id: "p-1",
  directories: [
    { source_id: "repo", name: "Repository", is_git: true },
    { source_id: "assets", name: "Assets", is_git: false },
  ],
};

const plainWorkspace = {
  id: "ws-1",
  project_id: "p-1",
  directories: [{
    source_id: "assets", name: "Assets", path: "/tmp/workspaces/ws-1/assets", is_git: false,
    source_path: "/srv/projects/assets", source_is_git: false,
  }],
};
const initOptions = {
  workspace_id: "ws-1", source_id: "assets",
  workspace: { path: "/tmp/workspaces/ws-1/assets", is_git: false, available: true },
  source: { path: "/srv/projects/assets", is_git: false, available: true },
};

// A workspace is a checkout on one machine, so every answer this surface reads
// comes from the machine the route names. The account has two other devices
// throughout: one paired and answering, one listed but never opened here.
const device = (deviceId, answer = async () => ({})) => {
  const call = vi.fn(answer);
  adoptDeviceSession({ ...fakeSession(deviceId), call });
  return call;
};

let elsewhere;

beforeEach(() => {
  document.body.innerHTML = '<div id="toolbar"><span id="tb-verb"></span></div><nav id="dir-rail"></nav><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  mountGitPane.mockClear();
  mountConsole.mockClear();
  mountAgentRail.mockClear();
  renderFilesTab.mockClear();
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
  App.devices = [
    { id: "dev-1", name: "this machine", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
    { id: "dev-3", name: "workshop", status: "offline" },
  ];
  elsewhere = device("dev-2");
});

afterEach(() => {
  // Nothing this surface does is ever asked of a machine the route does not
  // name, whichever pane, dialog or late answer does the asking.
  expect(elsewhere).not.toHaveBeenCalled();
  App.viewDispose?.();
  App.viewDispose = null;
  resetDeviceContexts();
});

describe("workspace surface", () => {
  it("bounds the unused conversation on its initial workspace detail read", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    const call = device("dev-1", async () => workspace);

    await renderWorkspace();

    // The machine the route names is asked, and asked for the smallest thread
    // window: the rail pages the conversation it opens on its own.
    expect(call).toHaveBeenCalledWith("workspace.get", { workspace_id: "ws-1", ...SMALLEST_THREAD_PAGE });
  });

  it("scopes Files to a directory while terminals stay workspace scoped", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async () => workspace);
    await renderWorkspace();
    expect(renderFilesTab.mock.calls[0][1].scope).toEqual({ workspace_id: "ws-1", source_id: "assets" });
    expect(App.routeLeaveGuard).toBe(renderFilesTab.mock.results[0].value.canLeave);
    renderFilesTab.mock.calls[0][1].onFileOpen("logo.svg");
    expect(App.route.file).toBe("logo.svg");
    expect(mountConsole).toHaveBeenCalledWith(expect.anything(), { kind: "workspace", workspaceId: "ws-1", deviceId: "dev-1" });
    expect(mountAgentRail).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({
      kind: "workspace", workspaceId: "ws-1", projectId: "p-1",
    }));
  });

  it("keeps the current ref and explains a checkout that would overwrite changes", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    const call = device("dev-1", async (method, params) => {
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
    document.querySelector("[data-refpicker-toggle]").click();
    document.querySelector("[data-ref-kind=tag]").click();
    document.querySelector('[data-ref="refs/tags/v1"]').click();
    await flush();
    expect(call).toHaveBeenCalledWith("git.checkout_ref", {
      workspace_id: "ws-1", source_id: "repo", full_ref: "refs/tags/v1",
    });
    expect(document.querySelector(".workspace-reftrigger-name").textContent).toBe("main");
    expect(document.querySelector(".workspace-referror").textContent).toContain("local changes would be overwritten");
  });

  it("restores Files and Changes tabs after a successful ref checkout remounts the Git pane", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    device("dev-1", async (method) => {
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
    document.querySelector("[data-refpicker-toggle]").click();
    document.querySelector("[data-ref-kind=tag]").click();
    document.querySelector('[data-ref="refs/tags/v1"]').click();
    await flush();
    await flush();
    expect(mountGitPane).toHaveBeenCalledTimes(2);
    const selection = mountAgentRail.mock.calls[0][1].selection;
    selection.set("second-agent");
    for (const [, options] of mountGitPane.mock.calls) {
      expect(options.agentSelection).toBe(selection);
      expect(options.agentSelection.scope()).toEqual({ agent_id: "second-agent" });
    }
    expect([...document.querySelectorAll("#dir-rail [data-tab]")].map((tab) => tab.dataset.tab)).toEqual(["changes", "files"]);
  });

  it("keeps the two faces on the shell's rail, never inside the pane it switches", async () => {
    // The reviewer's phone: the tabs used to be painted into the commit/file
    // list, which on a narrow viewport is a drawer — so they sat at the bottom
    // of something you had to open to reach them.
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    device("dev-1", async () => workspace);
    await renderWorkspace();
    await flush();
    expect(document.querySelector("#tabbody [data-tab]")).toBeNull();
    expect(document.querySelector("#dir-rail .dirtab.active").dataset.tab).toBe("changes");
    // The rail belongs to the surface standing on it: leaving hands the shell's
    // column back empty.
    App.viewDispose();
    App.viewDispose = null;
    expect(document.querySelector("#dir-rail").children).toHaveLength(0);
  });

  it("does not remount a Git pane after its source has been left", async () => {
    let finishCheckout;
    const checkout = new Promise((resolve) => { finishCheckout = resolve; });
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
    device("dev-1", async (method) => {
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
    document.querySelector("[data-refpicker-toggle]").click();
    document.querySelector("[data-ref-kind=tag]").click();
    document.querySelector('[data-ref="refs/tags/v1"]').click();
    App.viewDispose();
    finishCheckout({});
    await flush();
    expect(mountGitPane).toHaveBeenCalledTimes(1);
  });

  it("does not offer Finish for a ready workspace", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async () => ({ ...workspace, status: "ready" }));
    await renderWorkspace();
    expect(document.querySelector("[data-workspace-action]")).toBeNull();
  });

  it("refreshes a failed workspace pane after Retry without replacing its terminal console", async () => {
    const failedWorkspace = { ...workspace, status: "failed" };
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    const call = device("dev-1", async (method) => method === "workspace.retry" ? { ...workspace, status: "ready" } : failedWorkspace);
    await renderWorkspace();
    const firstGuard = App.routeLeaveGuard;
    expect(document.querySelector("[data-workspace-action]").textContent).toBe("Retry");
    document.querySelector("[data-workspace-action]").click();
    await flush();
    expect(call).toHaveBeenCalledWith("workspace.retry", { workspace_id: "ws-1" });
    expect(renderFilesTab).toHaveBeenCalledTimes(2);
    expect(App.routeLeaveGuard).not.toBe(firstGuard);
    expect(mountConsole).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[data-workspace-action]")).toBeNull();
    expect(document.querySelector(".workspace-action-status").textContent).toBe("Workspace ready.");
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
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async (method) => method === "workspace.retry" ? repaired : mixedWorkspace);
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

  it("initializes a workspace copy without remounting Files or navigating away", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files", file: "draft.md" };
    const call = device("dev-1", async (method) => method === "workspace.git_init_options" ? initOptions : method === "workspace.init_git" ? {
      workspace: { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] },
      outcomes: [{ target: "workspace", status: "initialized", is_git: true }],
      source: { source_id: "assets", path: "/srv/projects/assets", is_git: false },
    } : plainWorkspace);
    await renderWorkspace();
    const pane = renderFilesTab.mock.results[0].value;
    const guard = App.routeLeaveGuard;
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="workspace"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    await flush();
    expect(call).toHaveBeenCalledWith("workspace.init_git", { workspace_id: "ws-1", source_id: "assets", target: "workspace" });
    expect([...document.querySelectorAll("#dir-rail [data-tab]")].map((tab) => tab.dataset.tab)).toEqual(["changes", "files"]);
    expect(App.route).toMatchObject({ tab: "files", file: "draft.md" });
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    expect(pane.dispose).not.toHaveBeenCalled();
    expect(App.routeLeaveGuard).toBe(guard);
    expect(mountConsole).toHaveBeenCalledTimes(1);
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[data-init-git]").textContent).toContain("original source");
  });

  it("describes independent repositories and lets a failed target be retried", async () => {
    let calls = 0;
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    const call = device("dev-1", async (method) => {
      if (method === "workspace.git_init_options") return initOptions;
      if (method !== "workspace.init_git") return plainWorkspace;
      calls += 1;
      return calls === 1 ? {
        workspace: { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] },
        outcomes: [
          { target: "workspace", status: "initialized", is_git: true },
          { target: "source", status: "failed", is_git: false, error: "permission denied" },
        ],
        source: { source_id: "assets", path: "/srv/projects/assets", is_git: false },
      } : {
        workspace: { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] },
        outcomes: [{ target: "source", status: "initialized", is_git: true }],
        source: { source_id: "assets", path: "/srv/projects/assets", is_git: true },
      };
    });
    await renderWorkspace();
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="both"]').click();
    expect(document.querySelector(".workspace-init-copy").textContent).toContain("/tmp/workspaces/ws-1/assets");
    expect(document.querySelector(".workspace-init-source").textContent).toContain("/srv/projects/assets");
    expect(document.querySelector(".workspace-init-note").textContent).toContain("independent repositories");
    document.querySelector("[data-confirm-init-git]").click();
    await flush();
    expect(document.querySelector("[data-init-error]").textContent).toContain("Original source: permission denied");
    expect(document.querySelector("[data-confirm-init-git]").textContent).toBe("Retry original source");
    document.querySelector("[data-confirm-init-git]").click();
    await flush();
    expect(call).toHaveBeenLastCalledWith("workspace.init_git", { workspace_id: "ws-1", source_id: "assets", target: "source" });
  });

  it("ignores an initialization response after the workspace view is disposed", async () => {
    let finish;
    const pending = new Promise((resolve) => { finish = resolve; });
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async (method) => method === "workspace.git_init_options" ? initOptions : method === "workspace.init_git" ? pending : plainWorkspace);
    await renderWorkspace();
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="workspace"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    App.viewDispose();
    finish({ workspace: { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] }, outcomes: [] });
    await flush();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".modal-scrim")).toBeNull();
  });

  it("keeps both failures visible and retries both targets together", async () => {
    let attempts = 0;
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    const call = device("dev-1", async (method) => {
      if (method === "workspace.git_init_options") return initOptions;
      if (method !== "workspace.init_git") return plainWorkspace;
      attempts += 1;
      if (attempts === 1) return {
        workspace: plainWorkspace,
        source: { id: "assets", is_git: false },
        results: [
          { target: "workspace", status: "failed", is_git: false, error: "copy failed" },
          { target: "source", status: "failed", is_git: false, error: "source failed" },
        ],
      };
      return { workspace: plainWorkspace, source: { id: "assets", is_git: false }, results: [] };
    });
    await renderWorkspace();
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="both"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    await flush();
    expect(document.querySelector("[data-init-error]").textContent).toContain("Workspace copy: copy failed");
    expect(document.querySelector("[data-init-error]").textContent).toContain("Original source: source failed");
    expect(document.querySelector("[data-confirm-init-git]").textContent).toBe("Retry both");
    document.querySelector("[data-confirm-init-git]").click();
    await flush();
    expect(call).toHaveBeenLastCalledWith("workspace.init_git", { workspace_id: "ws-1", source_id: "assets", target: "both" });
  });

  it("can reopen initialization after Escape dismisses the dialog", async () => {
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    const call = device("dev-1", async (method) => method === "workspace.git_init_options" ? initOptions : plainWorkspace);
    await renderWorkspace();
    document.querySelector("[data-init-git]").click();
    await flush();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 220));
    expect(document.querySelector(".modal-scrim")).toBeNull();
    document.querySelector("[data-init-git]").click();
    await flush();
    expect(document.querySelector(".modal-workspace-init")).not.toBeNull();
    expect(call).toHaveBeenCalledTimes(3);
  });

  it("discovers a plain original source after reloading a Git workspace copy", async () => {
    const initializedCopy = { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] };
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async (method) => method === "workspace.git_init_options" ? {
      ...initOptions, workspace: { ...initOptions.workspace, is_git: true },
    } : initializedCopy);
    await renderWorkspace();
    await flush();
    expect(document.querySelector("[data-init-git]").textContent).toContain("original source");
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
  });

  it("offers and clears workspace reconciliation after an interrupted initialization", async () => {
    const initializedCopy = { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] };
    const reconciliationOptions = {
      ...initOptions,
      workspace: { ...initOptions.workspace, is_git: true, needs_reconciliation: true },
      source: { ...initOptions.source, is_git: true },
    };
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async (method) => method === "workspace.git_init_options" ? reconciliationOptions : method === "workspace.init_git" ? {
      workspace: initializedCopy, source: { id: "assets", is_git: true },
      results: [{ target: "workspace", status: "already_initialized", is_git: true }],
    } : initializedCopy);
    await renderWorkspace();
    await flush();
    expect(document.querySelector("[data-init-git]").textContent).toContain("Finish Git initialization");
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="workspace"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    await new Promise((resolve) => setTimeout(resolve, 220));
    expect(document.querySelector("[data-init-git]")).toBeNull();
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
  });

  // A late answer is published only where it was asked for. The machine the
  // route names is what says which surface that is: once the reader has moved
  // to another device's workspace, the answer this one asked for belongs to a
  // surface that is no longer on screen, whether or not the view was disposed.
  it("does not publish a Git initialization response once the route has moved to another machine", async () => {
    let finish;
    const pending = new Promise((resolve) => { finish = resolve; });
    device("dev-1", async (method) => method === "workspace.git_init_options" ? initOptions : method === "workspace.init_git" ? pending : plainWorkspace);
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    await renderWorkspace();
    document.querySelector("[data-init-git]").click();
    await flush();
    document.querySelector('[data-init-target="workspace"]').click();
    document.querySelector("[data-confirm-init-git]").click();
    App.route = { ...App.route, deviceId: "dev-2" };
    finish({
      workspace: { ...plainWorkspace, directories: [{ ...plainWorkspace.directories[0], is_git: true }] },
      source: { id: "assets", is_git: false }, results: [{ target: "workspace", status: "initialized", is_git: true }],
    });
    await flush();
    expect([...document.querySelectorAll("#dir-rail [data-tab]")].map((tab) => tab.dataset.tab)).toEqual(["files"]);
    expect(renderFilesTab).toHaveBeenCalledTimes(1);
  });

  // A machine that goes while its workspace is open is a different case from a
  // link that arrives at one: the reader is already standing on what it read,
  // and that stays. All that is missing is whose state it is.
  it("keeps what was read when that machine goes, and names the machine over it", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", async () => workspace);
    await renderWorkspace();
    await flush();

    setContextOffline("dev-1");

    expect(document.querySelector("#root > .device-strip").textContent).toContain("this machine");
    expect(document.getElementById("root").classList.contains("device-away")).toBe(true);
    expect(document.getElementById("tabbody")).toBeTruthy();
  });

  // The frozen sentence is a promise about what is on screen: this is what that
  // machine last said. One whose machine went before its first read landed has
  // nothing to be whose, and says the plain thing instead.
  it("says the machine cannot be opened while nothing has been painted yet", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    App.route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };
    device("dev-1", () => new Promise(() => {}));
    renderWorkspace();
    await flush();

    setContextOffline("dev-1");

    expect(document.querySelector("#root > .device-strip").textContent).toBe(
      "this machine isn't connected, so this can't be opened right now.",
    );
  });

  // A link can name a machine this client has never opened — another device's
  // workspace, pasted in. There is nothing to read under it, so the surface
  // says which machine is missing instead of asking it anything.
  it("names the machine when the route's device has no context, and asks nothing", async () => {
    App.route = { name: "workspace", deviceId: "dev-3", projectId: "p-1", workspaceId: "ws-1", sourceId: "assets", tab: "files" };

    await renderWorkspace();

    expect(document.querySelector("#root .empty").textContent).toContain("workshop");
    expect(renderFilesTab).not.toHaveBeenCalled();
    expect(mountConsole).not.toHaveBeenCalled();
  });
});
