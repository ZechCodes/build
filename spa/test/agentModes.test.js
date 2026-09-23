// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { deviceSettingsAddress } from "../src/core/settingsRecords.js";
import {
  AGENT_MODE_FAMILIES,
  AGENT_MODES,
  agentModesPanelHtml,
  mountAgentModes,
} from "../src/core/agentModes.js";

const settings = (claude = "headless", codex = "tui") => ({ agent_modes: { claude, codex } });
const select = (family) => document.querySelector(`[data-agent-mode="${family}"]`);
const status = (family) => document.querySelector(`[data-agent-mode-status="${family}"]`);
const error = () => document.querySelector("[data-agent-modes-error]");

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = agentModesPanelHtml();
});

describe("the agent-modes panel", () => {
  it("wires a cached enabled control while the refresh is still pending", async () => {
    await writeCached(deviceSettingsAddress("dev-1"), settings());
    const callRpc = vi.fn((method) => method === "settings.get"
      ? new Promise(() => {})
      : Promise.resolve(settings("tui", "tui")));
    void mountAgentModes(document.body, { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(select("claude").disabled).toBe(false));
    select("claude").value = "tui";
    select("claude").dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("settings.set", { agent_modes: { claude: "tui" } }));
    await vi.waitFor(() => expect(status("claude").textContent).toBe("Saved."));
  });
  it("has the small panelHtml/mount API and names both families and modes", () => {
    expect(AGENT_MODE_FAMILIES).toEqual([
      { id: "claude", label: "Claude Code" },
      { id: "codex", label: "Codex" },
    ]);
    expect(AGENT_MODES).toEqual([
      { id: "headless", label: "Headless" },
      { id: "tui", label: "TUI" },
    ]);
    expect(document.body.textContent).toContain("Agent modes");
    expect(document.body.textContent).toContain("Applies to new agents; existing agents keep their mode.");
    expect(select("claude").disabled).toBe(true);
    expect(select("codex").disabled).toBe(true);
  });

  it("loads and displays only bridge-confirmed choices", async () => {
    const callRpc = vi.fn(async () => settings());
    await mountAgentModes(document.body, { callRpc });

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(select("claude").value).toBe("headless");
    expect(select("codex").value).toBe("tui");
    expect([...select("claude").options].map((option) => option.textContent)).toEqual(["Headless", "TUI"]);
    expect(select("claude").disabled).toBe(false);
  });

  it("disables both choices and clearly explains an older bridge without agent_modes", async () => {
    await mountAgentModes(document.body, { callRpc: vi.fn(async () => ({ projects_dir: "/p" })) });

    expect(select("claude").disabled).toBe(true);
    expect(select("codex").disabled).toBe(true);
    expect([...select("claude").options].map((option) => option.textContent)).toEqual(["Unavailable"]);
    expect(error().textContent).toContain("unavailable on this bridge");
    expect(error().textContent).toContain("Update the bridge");
  });

  it("keeps a rejected settings.get local to the disabled panel", async () => {
    await mountAgentModes(document.body, {
      callRpc: vi.fn(async () => { throw new Error("device offline"); }),
    });

    expect(error().textContent).toBe("device offline");
    expect(select("claude").disabled).toBe(true);
    expect(select("claude").value).toBe("");
    expect(select("claude").textContent).toBe("Unavailable");
  });

  it("auto-saves only the changed family and paints the confirmed response", async () => {
    const callRpc = vi.fn(async (method) =>
      method === "settings.get" ? settings() : settings("tui", "tui"),
    );
    const onSaved = vi.fn();
    await mountAgentModes(document.body, { callRpc, onSaved });

    select("claude").value = "tui";
    select("claude").dispatchEvent(new Event("change"));
    expect(select("claude").disabled).toBe(true);
    expect(status("claude").textContent).toBe("Saving…");
    await vi.waitFor(() => expect(status("claude").textContent).toBe("Saved."));

    expect(callRpc).toHaveBeenCalledWith("settings.set", { agent_modes: { claude: "tui" } });
    expect(select("claude").value).toBe("tui");
    expect(select("claude").disabled).toBe(false);
    expect(status("claude").textContent).toBe("Saved.");
    expect(onSaved).toHaveBeenCalledWith(settings("tui", "tui"));
  });

  it("restores the confirmed choice and reports a rejected save", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return settings();
      throw new Error("cannot write config");
    });
    await mountAgentModes(document.body, { callRpc });

    select("codex").value = "headless";
    select("codex").dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(error().textContent).toBe("cannot write config"));

    expect(callRpc).toHaveBeenCalledWith("settings.set", { agent_modes: { codex: "headless" } });
    expect(select("codex").value).toBe("tui");
    expect(select("codex").disabled).toBe(false);
    expect(status("codex").textContent).toBe("");
    expect(error().textContent).toBe("cannot write config");
    expect(callRpc).toHaveBeenCalledTimes(3);
  });

  it("rejects an unconfirmed save response and restores the prior value", async () => {
    const callRpc = vi.fn(async (method) => method === "settings.get" ? settings() : { agent_modes: {} });
    await mountAgentModes(document.body, { callRpc });

    select("claude").value = "tui";
    select("claude").dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(error().textContent).toContain("agent_modes.claude"));

    expect(select("claude").value).toBe("headless");
    expect(error().textContent).toContain("agent_modes.claude");
  });

  it("keeps the cached choice when a rejected save cannot reload the bridge", async () => {
    let reads = 0;
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get" && reads++ === 0) return settings();
      if (method === "settings.get") throw new Error("device went offline");
      throw new Error("cannot write config");
    });
    await mountAgentModes(document.body, { callRpc });

    select("claude").value = "tui";
    select("claude").dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(error().textContent).toContain("device went offline"));

    expect(select("claude").disabled).toBe(false);
    expect(select("codex").disabled).toBe(false);
    expect(select("claude").value).toBe("headless");
    expect(error().textContent).toContain("cannot write config");
    expect(error().textContent).toContain("device went offline");
  });
});
