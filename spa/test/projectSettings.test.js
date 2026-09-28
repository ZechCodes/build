// @vitest-environment jsdom
// The ⋯ menu's Project settings sheet: what the bridge actually holds for a
// project (name, path, base branch) plus the one project-level mutation it
// exposes for an existing project — project.set_remote.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords, writeUiRecord, readUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";
import { projectSettingsAddress } from "../src/core/settingsRecords.js";

const confirmAction = vi.fn();
vi.mock("../src/core/confirm.js", () => ({ confirmAction: (...args) => confirmAction(...args) }));

const PROJECT = {
  project_id: "proj-1",
  name: "build",
  path: "/Users/z/Projects/build",
  base_branch: "main",
  remote: "git@github.com:example/build.git",
};

beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  confirmAction.mockReset();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

describe("openProjectSettings", () => {
  it("restores and updates an unsaved remote through the local cache", async () => {
    const address = uiAddress({ deviceId: "dev-1", entityId: PROJECT.project_id, view: "project-settings", kind: "draft" });
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    await writeUiRecord(address, { remote: "cached remote", source: null, focusId: "" });
    openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psremote")?.value).toBe("cached remote"));
    await writeUiRecord(address, { remote: "another tab remote", source: null, focusId: "" });
    await vi.waitFor(() => expect(document.querySelector("#psremote").value).toBe("another tab remote"));
    const remote = document.querySelector("#psremote");
    remote.value = "typed remote";
    remote.dispatchEvent(new Event("input"));
    await vi.waitFor(async () => expect((await readUiRecord(address))?.value.remote).toBe("typed remote"));
    document.querySelector("#pscancel").click();
    openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psremote")?.value).toBe("typed remote"));
  });
  it("keeps a focused remote draft when a delayed project.list repaints the sheet", async () => {
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    let answerList;
    const callRpc = vi.fn(() => new Promise((resolve) => { answerList = resolve; }));
    openProjectSettings(PROJECT.project_id, { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psremote")?.value).toBe(PROJECT.remote));
    const remote = document.querySelector("#psremote");
    remote.value = "my unsaved remote";
    remote.focus();
    remote.setSelectionRange(3, 3);
    answerList({ projects: [{ ...PROJECT, name: "from bridge" }] });
    await vi.waitFor(() => expect(document.querySelector("#psproject").value).toBe("from bridge"));
    expect(document.querySelector("#psremote").value).toBe("my unsaved remote");
    expect(document.activeElement).toBe(document.querySelector("#psremote"));
    expect(document.querySelector("#psremote").selectionStart).toBe(3);
  });

  it("keeps a focused remote draft through a project cache announcement", async () => {
    const address = projectSettingsAddress("dev-1", PROJECT.project_id);
    await writeCached(address, PROJECT);
    const sheet = openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psremote")?.value).toBe(PROJECT.remote));
    const remote = document.querySelector("#psremote");
    remote.value = "my unsaved remote";
    remote.focus();
    await writeCached(address, { ...PROJECT, name: "renamed" });
    await sheet.whenCachePainted();
    expect(document.querySelector("#psproject").value).toBe("renamed");
    expect(document.querySelector("#psremote").value).toBe("my unsaved remote");
    expect(document.activeElement).toBe(document.querySelector("#psremote"));
  });

  it("keeps an unfinished add-remote form when the project record changes", async () => {
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psaddremote")).toBeTruthy());
    document.querySelector("#psaddremote").click();
    document.querySelector("#psremoteurl").value = "git@github.com:example/draft.git";
    document.querySelector("#pssourcelabel").value = "draft";
    document.querySelector("#pssourcelabel").focus();
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), { ...PROJECT, name: "renamed" });
    await vi.waitFor(() => expect(document.querySelector("#psproject").value).toBe("renamed"));
    expect(document.querySelector("#psremoteurl").value).toBe("git@github.com:example/draft.git");
    expect(document.querySelector("#pssourcelabel").value).toBe("draft");
    expect(document.activeElement).toBe(document.querySelector("#pssourcelabel"));
  });

  it("keeps the folder browser open through a project record announcement", async () => {
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { projects_dir: "/Users/z/Projects" };
      if (method === "fs.list") return { path: "/Users/z/Projects", parent: "/Users/z", is_git: false, entries: [] };
      return new Promise(() => {});
    });
    const sheet = openProjectSettings(PROJECT.project_id, { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psaddfolder")).toBeTruthy());
    document.querySelector("#psaddfolder").click();
    await vi.waitFor(() => expect(document.querySelector("#psbrowseback")).toBeTruthy());
    const back = document.querySelector("#psbrowseback");
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), { ...PROJECT, name: "renamed" });
    await sheet.whenCachePainted();
    expect(document.querySelector("#psbrowseback")).toBe(back);
    back.click();
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe("renamed"));
  });
  it("paints the cached project while project.list has no answer", async () => {
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    const callRpc = vi.fn(() => new Promise(() => {}));
    openProjectSettings(PROJECT.project_id, { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe("build"));
    expect(callRpc).toHaveBeenCalledWith("project.list");
  });

  it("shows the project's identity read-only and its remote as the one editable field", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe("build"));
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector(":scope > .settings-sheet-frame > .settings-sheet-header h3").textContent).toBe("Project settings");
    expect(sheet.querySelector(".settings-sheet-body #psproject")).not.toBeNull();
    expect(sheet.querySelector("#psproject").value).toBe("build");
    expect(sheet.querySelector("#pspath").value).toBe("/Users/z/Projects/build");
    expect(sheet.querySelector("#psbranch").value).toBe("main");
    expect(sheet.querySelector("#psremote").value).toBe("git@github.com:example/build.git");
    expect(sheet.querySelector("#psproject").readOnly).toBe(true);
    expect(sheet.querySelector("#pspath").readOnly).toBe(true);
    expect(sheet.querySelector("#psbranch").readOnly).toBe(true);
  });

  it("asks only the caller it was handed", async () => {
    // Whoever opens the sheet has already resolved which machine this project
    // is on, and hands the sheet that machine's caller: the id it sends is the
    // bare one that machine's daemon minted, so asking anybody else would read
    // one device's project through another's bridge.
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psremote")).toBeTruthy());
    document.getElementById("psremote").value = "git@github.com:example/other.git";
    document.getElementById("pssave").click();
    await vi.waitFor(() => expect(document.querySelector("#scrim").classList.contains("show")).toBe(false));
    expect(callRpc).toHaveBeenCalledWith("project.set_remote", {
      project_id: "proj-1",
      url: "git@github.com:example/other.git",
    });
  });

  it("saves the remote through the bridge's own method and closes", async () => {
    const callRpc = vi.fn().mockImplementation((method) =>
      method === "project.list" ? Promise.resolve({ projects: [PROJECT] }) : Promise.resolve({}),
    );
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psremote")).toBeTruthy());
    document.getElementById("psremote").value = "git@github.com:example/other.git";
    document.getElementById("pssave").click();
    await vi.waitFor(() => expect(document.querySelector("#scrim").classList.contains("show")).toBe(false));
    expect(callRpc).toHaveBeenCalledWith("project.set_remote", {
      project_id: "proj-1",
      url: "git@github.com:example/other.git",
    });
    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
  });

  it("keeps the sheet open and names the failure when the save is rejected", async () => {
    const callRpc = vi.fn().mockImplementation((method) =>
      method === "project.list"
        ? Promise.resolve({ projects: [PROJECT] })
        : Promise.reject(new Error("not a git remote")),
    );
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#pssave")).toBeTruthy());
    document.getElementById("pssave").click();
    await vi.waitFor(() => expect(document.querySelector("#pserr").textContent).toContain("not a git remote"));
    expect(document.getElementById("pserr").textContent).toContain("not a git remote");
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
  });

  it("reports a project the bridge no longer knows rather than an empty sheet", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [] });
    openProjectSettings("proj-9", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#sheet").textContent).toContain("no longer"));
    expect(document.getElementById("sheet").textContent).toContain("no longer");
    expect(document.getElementById("psremote")).toBeNull();
  });

  // The one project-level choice about how work is checked out. The sheet
  // writes no RPC name, label or isolation word of its own: it mounts the
  // control the settings panel mounts, told to save on this project.
  it("offers the device default first, named after what the device holds", async () => {
    const callRpc = vi.fn().mockResolvedValue({
      projects: [{ ...PROJECT, isolation: null, isolation_default: "rift", isolation_available: { rift: true } }],
    });
    openProjectSettings("proj-1", { callRpc });

    await vi.waitFor(() => expect(document.querySelector("#sheet [data-isolation=select]")?.disabled).toBe(false));
    const select = document.querySelector("#sheet [data-isolation=select]");

    expect([...select.options].map((option) => option.value)).toEqual(["", "worktree", "rift"]);
    expect(select.options[0].textContent).toBe("Device default (Rift (copy-on-write))");
    expect(select.value).toBe("");
    expect(select.disabled).toBe(false);
  });

  it("saves this project's own isolation on change, and clears it the same way", async () => {
    const row = { ...PROJECT, isolation: null, isolation_default: "worktree", isolation_available: { rift: true } };
    const callRpc = vi.fn().mockImplementation((method, params) =>
      method === "project.list"
        ? Promise.resolve({ projects: [row] })
        : Promise.resolve({ ...row, isolation: params.isolation }),
    );
    openProjectSettings("proj-1", { callRpc });

    await vi.waitFor(() => expect(document.querySelector("#sheet [data-isolation=select]")?.disabled).toBe(false));
    const select = document.querySelector("#sheet [data-isolation=select]");

    select.value = "rift";
    select.dispatchEvent(new Event("change"));


    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "proj-1", isolation: "rift" });
    await vi.waitFor(() => {
      const current = document.querySelector("#sheet [data-isolation=select]");
      expect(current).not.toBe(select);
      expect(current.disabled).toBe(false);
      expect(current.value).toBe("rift");
    });
    const refreshed = document.querySelector("#sheet [data-isolation=select]");
    refreshed.value = "";
    refreshed.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "proj-1", isolation: null }));
    // The repaint draws a disabled "loading…" placeholder before the mount
    // fills it in, so wait for the filled select, not just a new node.
    await vi.waitFor(() => {
      const current = document.querySelector("#sheet [data-isolation=select]");
      expect(current).not.toBe(refreshed);
      expect(current.disabled).toBe(false);
      expect(current.value).toBe("");
    });
    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "proj-1", isolation: null });
  });

  it("shows why Rift is unavailable and does not offer it", async () => {
    const callRpc = vi.fn().mockResolvedValue({
      projects: [
        {
          ...PROJECT,
          isolation: null,
          isolation_default: "worktree",
          isolation_available: { rift: false, reason: "Rift CLI was not found" },
        },
      ],
    });
    openProjectSettings("proj-1", { callRpc });

    await vi.waitFor(() => expect(document.querySelector("#sheet [data-isolation=select]")?.disabled).toBe(false));

    expect([...document.querySelector("#sheet [data-isolation=select]").options].map((o) => o.disabled)).toEqual([
      false,
      false,
      true,
    ]);
    expect(document.querySelector("#sheet [data-isolation=lock]").textContent).toBe(
      "Rift is unavailable on this device: Rift CLI was not found.",
    );
  });

  it("keeps the remote's save separate from the isolation's refusal", async () => {
    const row = { ...PROJECT, isolation: null, isolation_default: "worktree", isolation_available: { rift: true } };
    const callRpc = vi.fn().mockImplementation((method) =>
      method === "project.list"
        ? Promise.resolve({ projects: [row] })
        : Promise.reject(new Error("Rift isolation is unavailable: Rift CLI was not found")),
    );
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#sheet [data-isolation=select]")?.disabled).toBe(false));
    const select = document.querySelector("#sheet [data-isolation=select]");

    select.value = "rift";
    select.dispatchEvent(new Event("change"));

    await vi.waitFor(() => expect(document.querySelector("#sheet [data-isolation=error]")?.textContent).toContain("Rift CLI was not found"));

    expect(document.querySelector("#sheet [data-isolation=error]").textContent).toContain("Rift CLI was not found");
    expect(document.getElementById("pserr").textContent).toBe("");
    expect(select.value).toBe("");
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
  });

  it("escapes what the project record carries", async () => {
    const callRpc = vi.fn().mockResolvedValue({
      projects: [{ ...PROJECT, name: '"><img src=x>', path: "<b>p</b>" }],
    });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe('"><img src=x>'));
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector("img")).toBeNull();
    expect(sheet.querySelector("b")).toBeNull();
    expect(sheet.querySelector("#psproject").value).toBe('"><img src=x>');
  });
});

describe("project deletion", () => {
  const caller = () => vi.fn(async (method) => method === "project.list" ? { projects: [PROJECT] } : {});

  it("requires confirmation and leaves the project untouched on cancel", async () => {
    confirmAction.mockResolvedValue(false);
    const callRpc = caller();
    openProjectSettings(PROJECT.project_id, { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psdelete")).toBeTruthy());
    document.querySelector("#psdelete").click();
    await vi.waitFor(() => expect(document.querySelector("#psdelete").disabled).toBe(false));
    expect(confirmAction).toHaveBeenCalledWith(expect.objectContaining({ danger: true, title: "Delete build?" }));
    expect(callRpc).not.toHaveBeenCalledWith("project.delete", expect.anything());
    expect(document.querySelector("#psdelete").disabled).toBe(false);
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
  });

  it("deletes through the project's device only after confirmation and refreshes its caller", async () => {
    confirmAction.mockResolvedValue(true);
    const callRpc = caller();
    const onDeleted = vi.fn();
    openProjectSettings(PROJECT.project_id, { callRpc, onDeleted });
    await vi.waitFor(() => expect(document.querySelector("#psdelete")).toBeTruthy());
    document.querySelector("#psdelete").click();
    await vi.waitFor(() => expect(onDeleted).toHaveBeenCalledWith(PROJECT));
    expect(callRpc).toHaveBeenCalledWith("project.delete", { project_id: PROJECT.project_id, confirm: true });
    expect(onDeleted).toHaveBeenCalledWith(PROJECT);
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
  });

  it("keeps a newer sheet open when an earlier deletion finishes", async () => {
    confirmAction.mockResolvedValue(true);
    let finishDelete;
    const callRpc = vi.fn((method) => method === "project.list"
      ? Promise.resolve({ projects: [PROJECT] })
      : new Promise((resolve) => { finishDelete = resolve; }));
    const onDeleted = vi.fn();
    openProjectSettings(PROJECT.project_id, { callRpc, onDeleted });
    await vi.waitFor(() => expect(document.querySelector("#psdelete")).toBeTruthy());
    document.querySelector("#psdelete").click();
    await vi.waitFor(() => expect(finishDelete).toBeTypeOf("function"));
    expect(document.querySelector("#pssave").disabled).toBe(true);
    document.querySelector("#sheet").innerHTML = "Another sheet";
    finishDelete({ deleted: true });
    await vi.waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(document.querySelector("#sheet").textContent).toBe("Another sheet");
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
  });

  it("shows deletion failures and allows retry without closing", async () => {
    confirmAction.mockResolvedValue(true);
    const callRpc = caller().mockImplementation(async (method) => {
      if (method === "project.list") return { projects: [PROJECT] };
      throw new Error("Workspace is busy");
    });
    openProjectSettings(PROJECT.project_id, { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psdelete")).toBeTruthy());
    document.querySelector("#psdelete").click();
    await vi.waitFor(() => expect(document.querySelector("#pserr").textContent).toBe("Workspace is busy"));
    expect(document.querySelector("#pserr").textContent).toBe("Workspace is busy");
    expect(document.querySelector("#psdelete").disabled).toBe(false);
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
  });
});

// The project's folders are what every new workspace is cut from, so the sheet
// that shows them is where one is added and where one is taken off.
describe("project sources", () => {
  const SOURCED = {
    ...PROJECT,
    sources: [
      { id: "source-1", name: "build", mount: "build", path: "/Users/z/Projects/build", is_git: true, base_branch: "main" },
      { id: "source-2", name: "assets", mount: "assets", path: "/Users/z/Projects/assets", is_git: false },
    ],
  };
  const caller = (answer = SOURCED) =>
    vi.fn(async (method) => (method === "project.list" ? { projects: [answer] } : answer));

  it("offers a remove beside every folder and repaints from what the bridge answers", async () => {
    const shrunk = { ...SOURCED, sources: [SOURCED.sources[0]] };
    const callRpc = vi.fn(async (method) => (method === "project.list" ? { projects: [SOURCED] } : shrunk));
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelectorAll("#sheet [data-remove-source]")).toHaveLength(2));
    expect(document.querySelectorAll("#sheet [data-remove-source]").length).toBe(2);

    document.querySelector('[data-remove-source="source-2"]').click();
    await vi.waitFor(() => expect(document.querySelectorAll("#sheet [data-remove-source]")).toHaveLength(1));

    expect(callRpc).toHaveBeenCalledWith("project.remove_source", { project_id: "proj-1", source_id: "source-2" });
    expect(document.querySelectorAll("#sheet [data-remove-source]").length).toBe(1);
  });

  it("says in the sheet why a removal was refused", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "project.list") return { projects: [SOURCED] };
      throw new Error("a project must have at least one source");
    });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector('[data-remove-source="source-2"]')).toBeTruthy());
    document.querySelector('[data-remove-source="source-2"]').click();
    await vi.waitFor(() => expect(document.querySelector("#pssrcerr").textContent).toContain("at least one source"));
    expect(document.getElementById("pssrcerr").textContent).toContain("at least one source");
    expect(document.querySelectorAll("#sheet [data-remove-source]").length).toBe(2);
  });

  it("adds a Git remote as a folder, under the name it was given", async () => {
    const callRpc = caller();
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psaddremote")).toBeTruthy());
    document.getElementById("psaddremote").click();
    document.getElementById("psremoteurl").value = "git@github.com:example/tokens.git";
    document.getElementById("pssourcelabel").value = "tokens";
    document.getElementById("pssourceadd").click();
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.add_source", expect.objectContaining({ remote: "git@github.com:example/tokens.git" })));

    expect(callRpc).toHaveBeenCalledWith("project.add_source", {
      project_id: "proj-1",
      remote: "git@github.com:example/tokens.git",
      name: "tokens",
    });
  });

  it("adds a folder on the device through the browser it opens", async () => {
    const callRpc = vi.fn(async (method, params) => {
      if (method === "project.list") return { projects: [SOURCED] };
      if (method === "settings.get") return { projects_dir: "/Users/z/Projects" };
      if (method === "fs.list") {
        return { path: "/Users/z/Projects", parent: "/Users/z", is_git: false, entries: [{ name: "docs", path: "/Users/z/Projects/docs", is_git: false, is_hidden: false }] };
      }
      return { ...SOURCED, sources: [...SOURCED.sources, { id: "source-3", name: "docs", mount: "docs", path: params.path, is_git: false }] };
    });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psaddfolder")).toBeTruthy());

    document.getElementById("psaddfolder").click();
    await vi.waitFor(() => expect(document.querySelector('#sheet .use[data-path="/Users/z/Projects/docs"]')).toBeTruthy());
    document.querySelector('#sheet .use[data-path="/Users/z/Projects/docs"]').click();
    await vi.waitFor(() => expect(document.querySelectorAll("#sheet [data-remove-source]")).toHaveLength(3));

    expect(callRpc).toHaveBeenCalledWith("project.add_source", { project_id: "proj-1", path: "/Users/z/Projects/docs" });
    expect(document.querySelectorAll("#sheet [data-remove-source]").length).toBe(3);
  });
});
