// @vitest-environment jsdom
// The cog's sheet on the view-area toolbar: renaming the workspace's pretty
// name, the agent defaults scoped to this workspace, and the delete that takes
// its checkouts with it.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { wipeUiRecords, writeUiRecord, readUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";
import { deviceModelsAddress, projectSettingsAddress, workspaceSettingsAddress } from "../src/core/settingsRecords.js";

const confirmAction = vi.fn();
vi.mock("../src/core/confirm.js", () => ({
  confirmAction: (...args) => confirmAction(...args),
  isConfirmOpen: () => false,
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({
  notifyError: (...args) => notifyError(...args),
  notifySuccess: () => {},
}));

const { openWorkspaceSettings } = await import("../src/sheets/workspaceSettings.js");
const {
  WORKSPACE_DEFAULTS_KEY,
  agentDefaultsForWorkspace,
  agentDefaultsInWorkspace,
  workspaceDefaultsStorage,
} = await import("../src/core/workspaceDefaults.js");
const { AGENT_DEFAULTS_KEY, saveDefaultHarness, saveHarnessDefault } = await import("../src/core/agentDefaults.js");

const WORKSPACE = { id: "ws-1", name: "payment-work", workspaceKey: "dev-1/ws-1" };

// The same catalog shape the account page's own test uses: two creatable
// families, each with a headless carrier.
const CATALOG = {
  default_provider: "claude_adk",
  agent_modes: { claude: "headless", codex: "headless" },
  providers: [
    {
      id: "claude_adk",
      label: "Claude Code",
      models: [
        { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
        { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
      ],
      efforts: ["low", "high"],
    },
    {
      id: "codex_app_server",
      label: "Codex",
      models: [{ id: "gpt-5.6-sol", label: "GPT-5.6-Sol", supports_effort: true, efforts: ["medium", "ultra"] }],
      efforts: ["medium", "ultra"],
    },
  ],
};

const $ = (selector) => document.querySelector(selector);
const shown = () => document.getElementById("scrim").classList.contains("show");
const type = (value) => {
  $("#wslabel").value = value;
  $("#wslabel").dispatchEvent(new Event("input"));
};
const pick = (selector, value) => {
  $(selector).value = value;
  $(selector).dispatchEvent(new Event("change", { bubbles: true }));
};
/** This workspace's own slot, as the account's savers write into it. */
const slot = () => workspaceDefaultsStorage("dev-1/ws-1");

beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  confirmAction.mockReset();
  notifyError.mockReset();
  localStorage.clear();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

// Every sheet a test opened is closed after it, and its last draft lands
// before the next test wipes the store. A sheet left open would flush its
// debounced draft into whichever test runs 180 ms later (#299).
const opened = [];
afterEach(async () => {
  await Promise.allSettled(opened.splice(0).map((close) => close()));
});

const open = (options = {}) => {
  const callRpc = options.callRpc || vi.fn().mockResolvedValue({});
  opened.push(openWorkspaceSettings(WORKSPACE, { catalog: CATALOG, ...options, callRpc }));
  return callRpc;
};

describe("the workspace settings sheet", () => {
  it("restores a cached name and directory form before either pull answers", async () => {
    const address = uiAddress({ deviceId: "dev-1", entityId: WORKSPACE.id, view: "workspace-settings", kind: "draft" });
    await writeCached(workspaceSettingsAddress("dev-1", WORKSPACE.id), { id: WORKSPACE.id, project_id: "proj-1", directories: [] });
    await writeUiRecord(address, { name: "cached rename", directory: { kind: "remote", remote: "git@example.com:team/repo.git", name: "repo", path: "" } });
    const pending = vi.fn(() => new Promise(() => {}));
    open({ callRpc: pending });
    await vi.waitFor(() => expect($("#wslabel").value).toBe("cached rename"));
    await vi.waitFor(() => expect($("#wsdirremote")?.value).toBe("git@example.com:team/repo.git"));
    expect($("#wsdirlabel").value).toBe("repo");
    expect(pending).toHaveBeenCalledWith("workspace.get", { workspace_id: WORKSPACE.id });
    await writeUiRecord(address, { name: "external rename", directory: { kind: "remote", remote: "git@example.com:team/other.git", name: "other", path: "" } });
    await vi.waitFor(() => expect($("#wslabel").value).toBe("external rename"));
    expect($("#wsdirremote").value).toBe("git@example.com:team/other.git");
    expect($("#wsdirlabel").value).toBe("other");
    type("my unsent rename");
    await vi.waitFor(async () => expect((await readUiRecord(address))?.value.name).toBe("my unsent rename"));
  });
  it("keeps restored directory inputs wired after the cache replaces their fields", async () => {
    const address = uiAddress({ deviceId: "dev-1", entityId: WORKSPACE.id, view: "workspace-settings", kind: "draft" });
    await writeCached(workspaceSettingsAddress("dev-1", WORKSPACE.id), { id: WORKSPACE.id, project_id: "proj-1", directories: [] });
    await writeUiRecord(address, { name: null, directory: { kind: "remote", remote: "git@example.com:old.git", name: "old", path: "" } });
    open({ callRpc: vi.fn(() => new Promise(() => {})) });
    await vi.waitFor(() => expect($("#wsdirremote")?.value).toBe("git@example.com:old.git"));
    const remote = $("#wsdirremote");
    remote.value = "git@example.com:new.git";
    remote.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.waitFor(async () => expect((await readUiRecord(address))?.value.directory.remote).toBe("git@example.com:new.git"));
    $("#wscancel").click();
    open({ callRpc: vi.fn(() => new Promise(() => {})) });
    await vi.waitFor(() => expect($("#wsdirremote")?.value).toBe("git@example.com:new.git"));
    expect($("#wsdirlabel").value).toBe("old");
  });
  it("updates its open picker on a catalog cache write without losing the workspace name draft", async () => {
    await writeCached(deviceModelsAddress("dev-1"), CATALOG);
    open({ callRpc: vi.fn(() => new Promise(() => {})) });
    await vi.waitFor(() => expect($("#wsdefmodel-claude_adk")?.textContent).toContain("Claude Opus 5"));
    type("unsaved name");
    $("#wsdefmodel-claude_adk").focus();
    await writeCached(deviceModelsAddress("dev-1"), {
      ...CATALOG,
      providers: CATALOG.providers.map((provider) => provider.id === "claude_adk"
        ? { ...provider, models: [...provider.models, { id: "claude-new", label: "Claude New", supports_effort: true }] }
        : provider),
    });
    await vi.waitFor(() => expect($("#wsdefmodel-claude_adk").textContent).toContain("Claude New"));
    expect($("#wslabel").value).toBe("unsaved name");
    expect(document.activeElement).toBe($("#wsdefmodel-claude_adk"));

    pick("#wsdefmodel-claude_adk", "claude-opus-5");
    $("#wsdefeffort-claude_adk").focus();
    await writeCached(deviceModelsAddress("dev-1"), {
      ...CATALOG,
      providers: CATALOG.providers.map((provider) => provider.id === "claude_adk"
        ? { ...provider, models: [...provider.models, { id: "claude-next", label: "Claude Next", supports_effort: true }] }
        : provider),
    });
    await vi.waitFor(() => expect($("#wsdefmodel-claude_adk").textContent).toContain("Claude Next"));
    expect(document.activeElement).toBe($("#wsdefeffort-claude_adk"));
    expect($("#wsdefmodel-claude_adk").value).toBe("claude-opus-5");
  });
  it("paints cached directories and offered sources while both pulls are absent", async () => {
    await writeCached(workspaceSettingsAddress("dev-1", WORKSPACE.id), {
      id: WORKSPACE.id,
      project_id: "proj-1",
      directories: [{ id: "dir-1", source_id: "source-1", name: "bridge", path: "/w/bridge" }],
    });
    await writeCached(projectSettingsAddress("dev-1", "proj-1"), {
      project_id: "proj-1",
      sources: [{ id: "source-1", name: "bridge" }, { id: "source-2", name: "spa" }],
    });
    const callRpc = vi.fn(() => new Promise(() => {}));
    open({ callRpc });
    await vi.waitFor(() => {
      expect($("[data-remove-directory]")?.dataset.removeDirectory).toBe("dir-1");
      expect([...$("#wsdiradd").options].map((option) => option.value)).toContain("source-2");
    });
    expect(callRpc).toHaveBeenCalledWith("workspace.get", { workspace_id: WORKSPACE.id });
  });

  it("opens on the workspace's own name, with Save off until it changes", async () => {
    open();

    expect(shown()).toBe(true);
    expect($("#sheet .settings-sheet-header h3").textContent).toBe("Workspace settings");
    expect($("#wslabel").value).toBe("payment-work");
    expect($("#wssave").disabled).toBe(true);

    type("payments");
    expect($("#wssave").disabled).toBe(false);
  });

  it("keeps Save off for a blank name, and for the same name typed again", async () => {
    open();

    type("   ");
    expect($("#wssave").disabled).toBe(true);
    type("  payment-work  ");
    expect($("#wssave").disabled).toBe(true);
  });

  it("renames on the caller it was handed, trimmed, then closes and refreshes", async () => {
    const onRenamed = vi.fn();
    const callRpc = open({ onRenamed });

    type("  payments  ");
    $("#wssave").click();
    await vi.waitFor(() => expect(onRenamed).toHaveBeenCalled());

    expect(callRpc).toHaveBeenCalledWith("workspace.rename", { workspace_id: "ws-1", name: "payments" });
    expect(onRenamed).toHaveBeenCalled();
    expect(shown()).toBe(false);
    const address = uiAddress({ deviceId: "dev-1", entityId: WORKSPACE.id, view: "workspace-settings", kind: "draft" });
    expect((await readUiRecord(address)).value.name).toBeNull();
  });

  it("says in the sheet why a rename was refused, and leaves it open to try again", async () => {
    const callRpc = vi.fn().mockRejectedValue(new Error("unknown workspace_id: ws-1"));
    open({ callRpc });

    type("payments");
    $("#wssave").click();
    await vi.waitFor(() => expect($("#wserr").textContent).toBe("unknown workspace_id: ws-1"));

    expect($("#wserr").textContent).toBe("unknown workspace_id: ws-1");
    expect(shown()).toBe(true);
    expect($("#wssave").disabled).toBe(false);
  });

  it("offers one defaults row per creatable harness, stored against this workspace alone", async () => {
    open();
    await vi.waitFor(() => expect(document.querySelectorAll("#sheet [data-harness]")).toHaveLength(2));
    const rows = [...document.querySelectorAll("#sheet [data-harness]")].map((row) => row.dataset.harness);
    expect(rows).toEqual(["claude_adk", "codex_app_server"]);

    pick("#wsdefmodel-codex_app_server", "gpt-5.6-sol");

    const stored = JSON.parse(localStorage.getItem(WORKSPACE_DEFAULTS_KEY));
    expect(stored["dev-1/ws-1"].harnesses.codex.model).toBe("gpt-5.6-sol");
    // A choice made inside one workspace never moves the account's own.
    expect(localStorage.getItem(AGENT_DEFAULTS_KEY)).toBe(null);
  });

  it("names the workspace and its checkouts in the confirmation, and deletes only on yes", async () => {
    confirmAction.mockResolvedValue(false);
    const callRpc = open();

    $("#wsdelete").click();
    await vi.waitFor(() => expect(confirmAction).toHaveBeenCalled());

    const asked = confirmAction.mock.calls[0][0];
    expect(asked.title).toBe("Delete payment-work?");
    expect(asked.warnings.join(" ")).toMatch(/checkouts on disk/);
    expect(asked.danger).toBe(true);
    expect(callRpc).not.toHaveBeenCalledWith("workspace.delete", expect.anything());
    expect($("#wsdelete").disabled).toBe(false);
  });

  it("deletes, forgets this workspace's stored defaults, closes and tells the caller", async () => {
    confirmAction.mockResolvedValue(true);
    const onDeleted = vi.fn();
    const callRpc = open({ onDeleted });
    await vi.waitFor(() => expect($("#wsdefmodel-codex_app_server")).toBeTruthy());
    pick("#wsdefmodel-codex_app_server", "gpt-5.6-sol");

    $("#wsdelete").click();
    await vi.waitFor(() => expect(onDeleted).toHaveBeenCalled());

    expect(callRpc).toHaveBeenCalledWith("workspace.delete", { workspace_id: "ws-1" });
    expect(JSON.parse(localStorage.getItem(WORKSPACE_DEFAULTS_KEY))["dev-1/ws-1"]).toBeUndefined();
    expect(shown()).toBe(false);
    expect(onDeleted).toHaveBeenCalled();
  });

  it("says why a delete was refused and keeps the workspace exactly as it was", async () => {
    confirmAction.mockResolvedValue(true);
    const callRpc = vi.fn().mockRejectedValue(new Error("Stop running agents before deleting the workspace"));
    const onDeleted = vi.fn();
    open({ callRpc, onDeleted });

    $("#wsdelete").click();
    await vi.waitFor(() => expect($("#wserr").textContent).toBe("Stop running agents before deleting the workspace"));

    expect($("#wserr").textContent).toBe("Stop running agents before deleting the workspace");
    expect(shown()).toBe(true);
    expect(onDeleted).not.toHaveBeenCalled();
  });
});

describe("the defaults a workspace layers over the account's", () => {
  it("reads the account's while the workspace names nothing", () => {
    saveDefaultHarness("claude_adk");
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" });
    expect(agentDefaultsForWorkspace("dev-1/ws-1", "claude_adk")).toEqual({
      provider: "claude_adk",
      model: "claude-opus-5",
      effort: "high",
    });
  });

  it("overrides a field at a time, leaving the rest falling through to the account", () => {
    saveHarnessDefault("claude_adk", { model: "claude-opus-5", effort: "high" });
    saveHarnessDefault("claude_adk", { model: "claude-haiku-4-5", effort: "" }, slot());
    expect(agentDefaultsForWorkspace("dev-1/ws-1", "claude_adk")).toEqual({
      provider: "claude_adk",
      model: "claude-haiku-4-5",
      effort: "high",
    });
  });

  it("lets the workspace name the harness a new agent leads with", () => {
    saveDefaultHarness("claude_adk");
    saveDefaultHarness("codex_app_server", slot());
    expect(agentDefaultsInWorkspace("dev-1/ws-1", CATALOG).provider).toBe("codex_app_server");
    // Outside a workspace nothing is layered: the account still leads.
    expect(agentDefaultsInWorkspace(null, CATALOG).provider).toBe("claude_adk");
  });

  it("is the account's, untouched, for a surface standing in no workspace", () => {
    saveHarnessDefault("codex_app_server", { model: "gpt-5.6-sol", effort: "ultra" });
    saveHarnessDefault("codex_app_server", { model: "never-read", effort: "" }, slot());
    expect(agentDefaultsForWorkspace(null, "codex_app_server")).toEqual({
      provider: "codex_app_server",
      model: "gpt-5.6-sol",
      effort: "ultra",
    });
  });
});

// The workspace's own folders: what it was cut with, plus whatever was added
// to it since. The project's sources are the offer, not the contents.
describe("workspace directories", () => {
  const DETAIL = {
    workspace_id: "ws-1",
    project_id: "proj-1",
    name: "payment-work",
    directories: [
      { id: "ws-1:source-1", source_id: "source-1", name: "bridge", path: "/w/ws-1/bridge", is_git: true, branch: "build/payment-work", status: "ready" },
    ],
  };
  const PROJECT = {
    project_id: "proj-1",
    name: "build",
    sources: [
      { id: "source-1", name: "bridge", mount: "bridge", is_git: true },
      { id: "source-2", name: "spa", mount: "spa", is_git: true },
    ],
  };
  const caller = (answer = DETAIL) =>
    vi.fn(async (method) => {
      if (method === "workspace.get") return DETAIL;
      if (method === "project.list") return { projects: [PROJECT] };
      return answer;
    });

  it("lists the directories the workspace holds, each with a way to remove it", async () => {
    const callRpc = caller();
    open({ callRpc });
    await vi.waitFor(() => expect($("#sheet [data-remove-directory]")).toBeTruthy());
    expect(callRpc).toHaveBeenCalledWith("workspace.get", { workspace_id: "ws-1" });
    expect($("#sheet [data-remove-directory]").dataset.removeDirectory).toBe("ws-1:source-1");
    expect($("#wsdirs").textContent).toContain("bridge");
  });

  it("removes one on the caller it was handed and repaints from the answer", async () => {
    const callRpc = caller({ ...DETAIL, directories: [] });
    open({ callRpc });
    await vi.waitFor(() => expect($("[data-remove-directory]")).toBeTruthy());
    $("[data-remove-directory]").click();
    await vi.waitFor(() => expect($("#sheet [data-remove-directory]")).toBeNull());
    expect(callRpc).toHaveBeenCalledWith("workspace.remove_directory", {
      workspace_id: "ws-1",
      directory_id: "ws-1:source-1",
    });
    expect($("#sheet [data-remove-directory]")).toBeNull();
  });

  it("offers the project's sources this workspace was not cut with, and adds the chosen one", async () => {
    const grown = { ...DETAIL, directories: [...DETAIL.directories, { id: "ws-1:source-2", source_id: "source-2", name: "spa", path: "/w/ws-1/spa", is_git: true, status: "ready" }] };
    const callRpc = caller(grown);
    open({ callRpc });
    await vi.waitFor(() => expect([...$("#wsdiradd").options].map((option) => option.value)).toContain("source-2"));
    const offered = [...$("#wsdiradd").options].map((option) => option.value);
    expect(offered).toContain("source-2");
    expect(offered).not.toContain("source-1");

    pick("#wsdiradd", "source-2");
    $("#wsdiraddgo").click();
    await vi.waitFor(() => expect($("#sheet").textContent).toContain("spa"));

    expect(callRpc).toHaveBeenCalledWith("workspace.add_directory", { workspace_id: "ws-1", source_id: "source-2" });
    expect($("#sheet").textContent).toContain("spa");
  });

  it("adds a Git remote as a directory of its own", async () => {
    const callRpc = caller();
    open({ callRpc });
    await vi.waitFor(() => expect($("#wsdiradd")).toBeTruthy());
    pick("#wsdiradd", "remote");
    $("#wsdirremote").value = "git@github.com:example/tokens.git";
    $("#wsdirlabel").value = "tokens";
    $("#wsdiraddgo").click();
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("workspace.add_directory", expect.objectContaining({ remote: "git@github.com:example/tokens.git" })));
    expect(callRpc).toHaveBeenCalledWith("workspace.add_directory", {
      workspace_id: "ws-1",
      remote: "git@github.com:example/tokens.git",
      name: "tokens",
    });
  });

  it("says in the sheet why a directory change was refused", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "workspace.get") return DETAIL;
      if (method === "project.list") return { projects: [PROJECT] };
      throw new Error("another filesystem operation is still running");
    });
    open({ callRpc });
    await vi.waitFor(() => expect($("[data-remove-directory]")).toBeTruthy());
    $("[data-remove-directory]").click();
    await vi.waitFor(() => expect($("#wsdirerr").textContent).toContain("still running"));
    expect($("#wsdirerr").textContent).toContain("still running");
    expect($("#sheet [data-remove-directory]")).not.toBeNull();
  });

  it("keeps a directory refusal visible when the project source list arrives later", async () => {
    let finishProjectList;
    const projectList = new Promise((resolve) => { finishProjectList = resolve; });
    const callRpc = vi.fn(async (method) => {
      if (method === "workspace.get") return DETAIL;
      if (method === "project.list") return projectList;
      throw new Error("another filesystem operation is still running");
    });
    open({ callRpc });
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("project.list"));
    $("[data-remove-directory]").click();
    await vi.waitFor(() => expect($("#wsdirerr").textContent).toContain("still running"));

    finishProjectList({ projects: [PROJECT] });
    await vi.waitFor(() => expect([...$("#wsdiradd").options].map((option) => option.value)).toContain("source-2"));
    expect($("#wsdirerr").textContent).toContain("still running");
  });
});
