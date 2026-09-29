// @vitest-environment jsdom
// The ⋯ menu's Project settings sheet: the project's name, a card per source
// (each with its own label, folder, base branch and remote), isolation, and
// deletion. A project has no remote of its own (#228): each source's is its
// checkout's origin, edited on that source's card.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords, writeUiRecord, readUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";
import { projectSettingsAddress } from "../src/core/settingsRecords.js";
import { sourceEditSupportAddress } from "../src/core/sourceEditSupport.js";

const confirmAction = vi.fn();
vi.mock("../src/core/confirm.js", () => ({ confirmAction: (...args) => confirmAction(...args) }));

const PROJECT = {
  project_id: "proj-1",
  name: "build",
  path: "/Users/z/Projects/build",
  base_branch: "main",
  remote: "git@github.com:example/build.git",
  sources: [
    { id: "source-1", name: "build", mount: "build", path: "/Users/z/Projects/build", is_git: true, base_branch: "main", remote: "git@github.com:example/build.git" },
    { id: "source-2", name: "docs", mount: "docs", path: "/Users/z/Projects/docs", is_git: true, base_branch: "main", remote: null },
  ],
};

const DRAFT = uiAddress({ deviceId: "dev-1", entityId: PROJECT.project_id, view: "project-settings", kind: "draft" });

/** A machine whose greeting named `project.update_source`. */
const editsInPlace = () => writeCached(sourceEditSupportAddress("dev-1"), { editsSources: true });

const card = (sourceId) => [...document.querySelectorAll("#sheet .ps-source")].find((element) => element.dataset.sourceId === sourceId);
const input = (sourceId, field) => card(sourceId)?.querySelector(`input[data-field="${field}"]`);
const type = (element, value) => {
  element.value = value;
  element.dispatchEvent(new Event("input"));
};

beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  confirmAction.mockReset();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

describe("openProjectSettings", () => {
  it("lays the sheet out as General, Sources, Isolation and Danger zone", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe("build"));
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector(":scope > .settings-sheet-frame > .settings-sheet-header h3").textContent).toBe("Project settings");
    expect([...sheet.querySelectorAll(".settings-sheet-body > .ps-section > h4")].map((h) => h.textContent))
      .toEqual(["General", "Sources", "Isolation", "Danger zone"]);
    expect(sheet.querySelector("#psdelete").closest(".ps-section").querySelector("h4").textContent).toBe("Danger zone");
    expect(sheet.querySelector("[data-isolation=select]").closest(".ps-section").querySelector("h4").textContent).toBe("Isolation");
  });

  it("shows the project's name read-only, as the project's and not a source's, and no project-level remote", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe("build"));
    const general = document.querySelector("#psproject").closest(".ps-section");
    expect(general.querySelector("label[for=psproject]").textContent).toBe("Project name");
    expect(document.querySelector("#psproject").readOnly).toBe(true);
    expect(general.querySelector(".ps-source")).toBeNull();
    expect(document.querySelector("#psremote")).toBeNull();
    expect(document.querySelector("#pspath")).toBeNull();
    expect(document.querySelector("#psbranch")).toBeNull();
    expect(document.querySelector("#sheet").textContent).not.toContain("Origin remote");
  });

  it("gives every source a card holding its label, folder, base branch, remote and remove", async () => {
    await editsInPlace();
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "name")?.readOnly).toBe(false));
    expect(document.querySelectorAll("#sheet .ps-source")).toHaveLength(2);
    expect(input("source-1", "remote").value).toBe("git@github.com:example/build.git");
    expect(input("source-2", "remote").value).toBe("");
    expect(input("source-2", "path").value).toBe("/Users/z/Projects/docs");
    expect(input("source-2", "base_branch").value).toBe("main");
    expect(input("source-1", "path").readOnly).toBe(true);
    expect(card("source-1").textContent).toContain("project's home");
    expect(input("source-2", "path").readOnly).toBe(false);
    expect(card("source-2").querySelector('[data-remove-source="source-2"]')).not.toBeNull();
    expect(card("source-2").querySelector('[data-save-source="source-2"]').disabled).toBe(true);
    expect(card("source-2").querySelector(".ps-source-tag").textContent).toBe("Git repository");
  });

  it("shows a plain folder without a base branch or remote", async () => {
    await editsInPlace();
    const folder = { ...PROJECT, sources: [PROJECT.sources[0], { id: "source-3", name: "assets", mount: "assets", path: "/a", is_git: false }] };
    openProjectSettings("proj-1", { callRpc: vi.fn().mockResolvedValue({ projects: [folder] }), deviceId: "dev-1" });
    await vi.waitFor(() => expect(card("source-3")).toBeTruthy());
    expect(input("source-3", "base_branch")).toBeNull();
    expect(input("source-3", "remote")).toBeNull();
    expect(card("source-3").querySelector(".ps-source-tag").textContent).toBe("Folder");
  });

  it("saves only what changed on a card through project.update_source, and says how many checkouts followed", async () => {
    await editsInPlace();
    const saved = {
      ...PROJECT,
      sources: [PROJECT.sources[0], { ...PROJECT.sources[1], name: "Docs", remote: "git@github.com:example/docs.git" }],
      checkouts_updated: 2,
    };
    const callRpc = vi.fn(async (method) => (method === "project.list" ? { projects: [PROJECT] } : saved));
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "name")?.readOnly).toBe(false));
    type(input("source-2", "name"), " Docs ");
    type(input("source-2", "remote"), "git@github.com:example/docs.git");
    const save = card("source-2").querySelector("[data-save-source]");
    expect(save.disabled).toBe(false);
    save.click();
    await vi.waitFor(() => expect(card("source-2").querySelector("[data-source-status]").textContent).toContain("2 existing workspace checkouts"));
    expect(callRpc).toHaveBeenCalledWith("project.update_source", {
      project_id: "proj-1",
      source_id: "source-2",
      name: "Docs",
      remote: "git@github.com:example/docs.git",
    });
    expect(input("source-2", "name").value).toBe("Docs");
    expect(card("source-2").querySelector("[data-save-source]").disabled).toBe(true);
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
  });

  it("keeps the card's edits and names the refusal on that card", async () => {
    await editsInPlace();
    const callRpc = vi.fn(async (method) => {
      if (method === "project.list") return { projects: [PROJECT] };
      throw new Error("A Git remote cannot start with a dash.");
    });
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "remote")?.readOnly).toBe(false));
    type(input("source-2", "remote"), "--upload-pack=sh");
    card("source-2").querySelector("[data-save-source]").click();
    await vi.waitFor(() => expect(card("source-2").querySelector("[data-source-error]").textContent).toContain("dash"));
    expect(input("source-2", "remote").value).toBe("--upload-pack=sh");
    expect(card("source-1").querySelector("[data-source-error]").textContent).toBe("");
    expect(card("source-2").querySelector("[data-save-source]").disabled).toBe(false);
  });

  it("on a bridge without project.update_source, edits only the first source's remote, through project.set_remote", async () => {
    const callRpc = vi.fn(async (method) => (method === "project.list" ? { projects: [PROJECT] } : PROJECT));
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-1", "remote")).toBeTruthy());
    expect(input("source-1", "remote").readOnly).toBe(false);
    expect(input("source-1", "name").readOnly).toBe(true);
    expect(input("source-2", "remote").readOnly).toBe(true);
    expect(input("source-2", "base_branch").readOnly).toBe(true);
    expect(card("source-2").querySelector("[data-save-source]")).toBeNull();
    expect(document.querySelector("#sheet").textContent).toContain("Update Build on this device");
    type(input("source-1", "remote"), "git@github.com:example/other.git");
    card("source-1").querySelector("[data-save-source]").click();
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.set_remote", {
      project_id: "proj-1",
      url: "git@github.com:example/other.git",
    }));
  });

  it("offers the in-place edits as soon as a greeting says the machine takes them", async () => {
    const callRpc = vi.fn(async () => ({ projects: [PROJECT] }));
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "name")?.readOnly).toBe(true));
    await editsInPlace();
    await vi.waitFor(() => expect(input("source-2", "name").readOnly).toBe(false));
  });

  it("restores and updates an unsaved card edit through the local cache", async () => {
    await editsInPlace();
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    await writeUiRecord(DRAFT, { edits: { "source-2": { remote: "cached remote" } }, source: null, focusId: "" });
    openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "remote")?.value).toBe("cached remote"));
    expect(card("source-2").querySelector("[data-save-source]").disabled).toBe(false);
    await writeUiRecord(DRAFT, { edits: { "source-2": { remote: "another tab remote" } }, source: null, focusId: "" });
    await vi.waitFor(() => expect(input("source-2", "remote").value).toBe("another tab remote"));
    type(input("source-2", "remote"), "typed remote");
    await vi.waitFor(async () => expect((await readUiRecord(DRAFT))?.value.edits["source-2"].remote).toBe("typed remote"));
    document.querySelector("#pscancel").click();
    openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "remote")?.value).toBe("typed remote"));
  });

  it("keeps a focused card edit when a delayed project.list repaints the sheet", async () => {
    await editsInPlace();
    await writeCached(projectSettingsAddress("dev-1", PROJECT.project_id), PROJECT);
    let answerList;
    const callRpc = vi.fn(() => new Promise((resolve) => { answerList = resolve; }));
    openProjectSettings(PROJECT.project_id, { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "remote")?.readOnly).toBe(false));
    const remote = input("source-2", "remote");
    remote.value = "my unsaved remote";
    remote.focus();
    remote.setSelectionRange(3, 3);
    answerList({ projects: [{ ...PROJECT, name: "from bridge" }] });
    await vi.waitFor(() => expect(document.querySelector("#psproject").value).toBe("from bridge"));
    expect(input("source-2", "remote").value).toBe("my unsaved remote");
    expect(document.activeElement).toBe(input("source-2", "remote"));
    expect(input("source-2", "remote").selectionStart).toBe(3);
  });

  it("keeps a focused card edit through a project cache announcement", async () => {
    await editsInPlace();
    const address = projectSettingsAddress("dev-1", PROJECT.project_id);
    await writeCached(address, PROJECT);
    const sheet = openProjectSettings(PROJECT.project_id, { callRpc: vi.fn(() => new Promise(() => {})), deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "name")?.readOnly).toBe(false));
    const name = input("source-2", "name");
    name.value = "my unsaved label";
    name.focus();
    await writeCached(address, { ...PROJECT, name: "renamed" });
    await sheet.whenCachePainted();
    expect(document.querySelector("#psproject").value).toBe("renamed");
    expect(input("source-2", "name").value).toBe("my unsaved label");
    expect(document.activeElement).toBe(input("source-2", "name"));
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

  it("asks only the caller it was handed", async () => {
    // Whoever opens the sheet has already resolved which machine this project
    // is on, and hands the sheet that machine's caller: the id it sends is the
    // bare one that machine's daemon minted, so asking anybody else would read
    // one device's project through another's bridge.
    await editsInPlace();
    const callRpc = vi.fn(async (method) => (method === "project.list" ? { projects: [PROJECT] } : PROJECT));
    openProjectSettings("proj-1", { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(input("source-2", "base_branch")?.readOnly).toBe(false));
    type(input("source-2", "base_branch"), "develop");
    card("source-2").querySelector("[data-save-source]").click();
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.update_source", {
      project_id: "proj-1",
      source_id: "source-2",
      base_branch: "develop",
    }));
  });

  it("reports a project the bridge no longer knows rather than an empty sheet", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [] });
    openProjectSettings("proj-9", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#sheet").textContent).toContain("no longer"));
    expect(document.getElementById("sheet").textContent).toContain("no longer");
    expect(document.querySelector("#sheet .ps-source")).toBeNull();
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

  it("keeps the project's errors separate from the isolation's refusal", async () => {
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
    const hostile = { id: 'x"><i>', name: '"><img src=x>', mount: "m", path: "<b>p</b>", is_git: true, base_branch: "<u>b</u>", remote: '"><svg onload=x>' };
    const callRpc = vi.fn().mockResolvedValue({
      projects: [{ ...PROJECT, name: '"><img src=x>', path: "<b>p</b>", sources: [PROJECT.sources[0], hostile] }],
    });
    openProjectSettings("proj-1", { callRpc });
    await vi.waitFor(() => expect(document.querySelector("#psproject")?.value).toBe('"><img src=x>'));
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector("img")).toBeNull();
    expect(sheet.querySelector("b")).toBeNull();
    expect(sheet.querySelector("i, u, svg")).toBeNull();
    expect(sheet.querySelector("#psproject").value).toBe('"><img src=x>');
    expect(input(hostile.id, "remote").value).toBe('"><svg onload=x>');
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
    expect(document.querySelector("#psaddfolder").disabled).toBe(true);
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
    await vi.waitFor(() => expect(document.querySelectorAll("#sheet .ps-source")).toHaveLength(1));

    expect(callRpc).toHaveBeenCalledWith("project.remove_source", { project_id: "proj-1", source_id: "source-2" });
    // A project keeps at least one source, so its last one offers no Remove.
    expect(document.querySelectorAll("#sheet [data-remove-source]").length).toBe(0);
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
