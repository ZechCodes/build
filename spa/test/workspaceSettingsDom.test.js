// @vitest-environment jsdom
// The cog's sheet on the view-area toolbar: renaming the workspace's pretty
// name, the agent defaults scoped to this workspace, and the delete that takes
// its checkouts with it.

import { describe, it, expect, vi, beforeEach } from "vitest";

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

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const $ = (selector) => document.querySelector(selector);
const shown = () => document.getElementById("scrim").classList.contains("show");
const type = (value) => {
  $("#wsname").value = value;
  $("#wsname").dispatchEvent(new Event("input"));
};
const pick = (selector, value) => {
  $(selector).value = value;
  $(selector).dispatchEvent(new Event("change", { bubbles: true }));
};
/** This workspace's own slot, as the account's savers write into it. */
const slot = () => workspaceDefaultsStorage("dev-1/ws-1");

beforeEach(() => {
  confirmAction.mockReset();
  notifyError.mockReset();
  localStorage.clear();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

const open = (options = {}) => {
  const callRpc = options.callRpc || vi.fn().mockResolvedValue({});
  openWorkspaceSettings(WORKSPACE, { catalog: CATALOG, ...options, callRpc });
  return callRpc;
};

describe("the workspace settings sheet", () => {
  it("opens on the workspace's own name, with Save off until it changes", async () => {
    open();
    await flush();
    expect(shown()).toBe(true);
    expect($("#sheet .settings-sheet-header h3").textContent).toBe("Workspace settings");
    expect($("#wsname").value).toBe("payment-work");
    expect($("#wssave").disabled).toBe(true);

    type("payments");
    expect($("#wssave").disabled).toBe(false);
  });

  it("keeps Save off for a blank name, and for the same name typed again", async () => {
    open();
    await flush();
    type("   ");
    expect($("#wssave").disabled).toBe(true);
    type("  payment-work  ");
    expect($("#wssave").disabled).toBe(true);
  });

  it("renames on the caller it was handed, trimmed, then closes and refreshes", async () => {
    const onRenamed = vi.fn();
    const callRpc = open({ onRenamed });
    await flush();
    type("  payments  ");
    $("#wssave").click();
    await flush();

    expect(callRpc).toHaveBeenCalledWith("workspace.rename", { workspace_id: "ws-1", name: "payments" });
    expect(onRenamed).toHaveBeenCalled();
    expect(shown()).toBe(false);
  });

  it("says in the sheet why a rename was refused, and leaves it open to try again", async () => {
    const callRpc = vi.fn().mockRejectedValue(new Error("unknown workspace_id: ws-1"));
    open({ callRpc });
    await flush();
    type("payments");
    $("#wssave").click();
    await flush();

    expect($("#wserr").textContent).toBe("unknown workspace_id: ws-1");
    expect(shown()).toBe(true);
    expect($("#wssave").disabled).toBe(false);
  });

  it("offers one defaults row per creatable harness, stored against this workspace alone", async () => {
    open();
    await flush();
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
    await flush();
    $("#wsdelete").click();
    await flush();

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
    await flush();
    pick("#wsdefmodel-codex_app_server", "gpt-5.6-sol");

    $("#wsdelete").click();
    await flush();

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
    await flush();
    $("#wsdelete").click();
    await flush();

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
