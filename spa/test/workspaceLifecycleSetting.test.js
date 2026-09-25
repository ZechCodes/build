/** @vitest-environment jsdom */
// The workspace lifecycle's two device settings (#167): how long a workspace
// goes without activity before it is quiet, and whether a quiet workspace
// loses its build output. The bridge holds both, because its reclaim service
// is what acts on them while no browser is open; an environment variable on
// the machine still wins, and the panel says so.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { deviceSettingsAddress } from "../src/core/settingsRecords.js";
import {
  mountWorkspaceLifecycleSetting,
  workspaceLifecycleOf,
  workspaceLifecyclePanelHtml,
} from "../src/core/workspaceLifecycleSetting.js";

const host = () => document.querySelector("#panels");
const hours = () => host().querySelector("#workspaceidlehours");
const prune = () => host().querySelector("#workspaceprune");
const text = (selector) => host().querySelector(selector).textContent;
const controlsShown = () => !host().querySelector("[data-lifecycle-controls]").hidden;

const DAY = { workspace_idle_secs: 86_400, workspace_prune: false, workspace_pinned: [] };

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = `<div id="panels">${workspaceLifecyclePanelHtml()}</div>`;
});

describe("what the bridge's answer means", () => {
  it("reads the threshold in hours, the switch, and what the environment pins", () => {
    expect(workspaceLifecycleOf(DAY)).toEqual({
      known: true,
      idleHours: 24,
      prune: false,
      idlePinned: false,
      prunePinned: false,
    });
    expect(workspaceLifecycleOf({
      workspace_idle_secs: 5400,
      workspace_prune: true,
      workspace_pinned: ["workspace_idle_secs", "workspace_prune"],
    })).toEqual({ known: true, idleHours: 1.5, prune: true, idlePinned: true, prunePinned: true });
  });

  // A bridge from before 1.25.0 answers settings.get without the fields.
  it("is unknown on a bridge that predates the settings, and before any answer", () => {
    expect(workspaceLifecycleOf({ watch_agent_filed_issues: true }).known).toBe(false);
    expect(workspaceLifecycleOf(null).known).toBe(false);
    expect(workspaceLifecycleOf(undefined).known).toBe(false);
  });
});

describe("the panel", () => {
  it("paints the machine's answer", async () => {
    const callRpc = vi.fn(async () => DAY);
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(hours().value).toBe("24");
    expect(hours().disabled).toBe(false);
    expect(prune().checked).toBe(false);
    expect(prune().disabled).toBe(false);
    expect(text("#workspaceidlepinned")).toBe("");
  });

  // Render from cache: what this device last said paints before, and without,
  // any answer on the connection.
  it("paints from the cached settings before the machine answers", async () => {
    await writeCached(deviceSettingsAddress("dev-1"), { ...DAY, workspace_idle_secs: 7200 });
    let answer;
    const callRpc = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    const mounted = mountWorkspaceLifecycleSetting(host(), { callRpc, deviceId: "dev-1" });
    await vi.waitFor(() => expect(hours().value).toBe("2"));
    expect(hours().disabled).toBe(false);
    answer(DAY);
    await mounted;
    expect(hours().value).toBe("24");
  });

  it("says in a sentence when the machine's bridge predates the settings", async () => {
    const callRpc = vi.fn(async () => ({ watch_agent_filed_issues: false }));
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    expect(controlsShown()).toBe(false);
    expect(text("#workspacelifecycleolder")).toBe(
      "This machine's bridge is older than these settings. Update it to change them here.",
    );
  });

  it("names the variable that pins a setting and leaves that control still", async () => {
    const callRpc = vi.fn(async () => ({ ...DAY, workspace_pinned: ["workspace_prune"] }));
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    expect(prune().disabled).toBe(true);
    expect(text("#workspaceprunepinned")).toBe("BRIDGE_WORKSPACE_PRUNE sets this on this machine.");
    expect(hours().disabled).toBe(false);
    expect(text("#workspaceidlepinned")).toBe("");
  });

  it("saves the threshold in seconds and repaints from the bridge's answer", async () => {
    const callRpc = vi.fn(async (method, params) =>
      method === "settings.set" ? { ...DAY, ...params } : DAY);
    const onSaved = vi.fn();
    await mountWorkspaceLifecycleSetting(host(), { callRpc, onSaved });

    hours().value = "6";
    await hours().onchange();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { workspace_idle_secs: 21_600 });
    expect(hours().value).toBe("6");
    expect(text("#workspacelifecyclesaved")).toBe("Saved");
    expect(onSaved).toHaveBeenCalled();
  });

  it("saves the switch", async () => {
    const callRpc = vi.fn(async (method, params) =>
      method === "settings.set" ? { ...DAY, ...params } : DAY);
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    prune().checked = true;
    await prune().onchange();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { workspace_prune: true });
    expect(prune().checked).toBe(true);
  });

  it("refuses a threshold that is not above zero without asking the machine", async () => {
    const callRpc = vi.fn(async () => DAY);
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    hours().value = "0";
    await hours().onchange();

    expect(callRpc).not.toHaveBeenCalledWith("settings.set", expect.anything());
    expect(text("#workspacelifecycleerr")).toBe("Enter a number of hours above zero.");
    expect(hours().value).toBe("24");
  });

  // The controls never show a choice the machine did not take.
  it("puts the controls back and says why when the save is refused", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.set") throw new Error("workspace_idle_secs must be a whole number of seconds above 0.");
      return DAY;
    });
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    hours().value = "3";
    await hours().onchange();

    expect(hours().value).toBe("24");
    expect(text("#workspacelifecycleerr")).toBe("workspace_idle_secs must be a whole number of seconds above 0.");
  });

  it("offers no control when the machine cannot be read and nothing is cached", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("no session");
    });
    await mountWorkspaceLifecycleSetting(host(), { callRpc });

    expect(hours().disabled).toBe(true);
    expect(prune().disabled).toBe(true);
    expect(text("#workspacelifecycleerr")).toBe("no session");
  });
});
