// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { deviceModelsAddress, deviceSettingsAddress, watchSettingsRecord } from "../src/core/settingsRecords.js";
import { agentModesPanelHtml, mountAgentModes } from "../src/core/agentModes.js";
import { defaultHarnessPanelHtml, mountDefaultHarness } from "../src/core/defaultHarness.js";
import { projectAgentPanelHtml, mountProjectAgentSetting } from "../src/core/projectAgentSetting.js";
import { agentRolesPanelHtml, mountAgentRoles } from "../src/core/agentRolesPanel.js";
import { watchSettingPanelHtml, mountWatchSetting } from "../src/core/watchSetting.js";
import { DEVICE_ISOLATION, isolationPanelHtml, mountIsolation } from "../src/core/isolation.js";

const DEVICE = "cached-device";
const SETTINGS = {
  default_harness: "codex",
  project_agent: { provider: "codex", model: "gpt-5", effort: "medium" },
  agent_modes: { claude: "tui", codex: "headless" },
  role_models: [{ model: "gpt-5", roles: ["reviewer"], capability: "scoped" }],
  watch_agent_filed_issues: false,
  isolation: "rift",
  isolation_available: { rift: true },
};
const CATALOG = { providers: [{ id: "codex", label: "Codex", models: [{ id: "gpt-5", label: "GPT 5", supports_effort: true }], efforts: ["medium"] }] };
const pending = vi.fn(() => new Promise(() => {}));

beforeEach(async () => {
  await wipeCache();
  pending.mockClear();
  document.body.innerHTML = "";
  await writeCached(deviceSettingsAddress(DEVICE), SETTINGS);
  await writeCached(deviceModelsAddress(DEVICE), CATALOG);
});

describe("bridge settings cached mounts", () => {
  it("can finish an owned cache read after the global document goes away", async () => {
    const owner = document.createElement("div");
    document.body.append(owner);
    const savedDocument = globalThis.document;
    let record;
    try {
      globalThis.document = undefined;
      record = watchSettingsRecord(deviceSettingsAddress(DEVICE), () => {}, { owner });
      await record.read();
    } finally {
      record?.dispose();
      globalThis.document = savedDocument;
    }
  });

  it.each([
    ["agent modes", agentModesPanelHtml, mountAgentModes, "#agentmode-claude", "tui"],
    ["fallback harness", defaultHarnessPanelHtml, mountDefaultHarness, "#defaultharness", "codex"],
    ["project agent", projectAgentPanelHtml, mountProjectAgentSetting, "#projectagentharness", "codex"],
    ["roles", agentRolesPanelHtml, mountAgentRoles, "[data-aroles-rows] tr th", "gpt-5"],
    ["watching", watchSettingPanelHtml, mountWatchSetting, "#watchagentissues", "false"],
    ["isolation", isolationPanelHtml, (host, options) => mountIsolation(host, { ...options, target: DEVICE_ISOLATION }), "[data-isolation=select]", "rift"],
  ])("paints cached %s while settings.get is absent", async (_name, html, mount, selector, shown) => {
    document.body.innerHTML = html();
    void mount(document.body, { callRpc: pending, deviceId: DEVICE });
    await vi.waitFor(() => {
      const control = document.querySelector(selector);
      expect(control).toBeTruthy();
      expect(control.type === "checkbox" ? String(control.checked) : (control.value ?? control.textContent)).toContain(shown);
    });
    expect(pending).toHaveBeenCalled();
  });

  it("repaints from a real cache announcement and readback", async () => {
    document.body.innerHTML = agentModesPanelHtml();
    void mountAgentModes(document.body, { callRpc: pending, deviceId: DEVICE });
    await vi.waitFor(() => expect(document.querySelector("#agentmode-claude").value).toBe("tui"));
    await writeCached(deviceSettingsAddress(DEVICE), { ...SETTINGS, agent_modes: { claude: "headless", codex: "headless" } });
    await vi.waitFor(() => expect(document.querySelector("#agentmode-claude").value).toBe("headless"));
    expect((await readCached(deviceSettingsAddress(DEVICE))).value.agent_modes.claude).toBe("headless");
  });
});
