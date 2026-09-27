/** @vitest-environment jsdom */
// The one switch watching needs (#65): whether a task an agent files is one
// the user hears about.
//
// It is the DEVICE's, beside the other bridge settings, because the bridge is
// what decides whether to watch at the moment an agent files — no browser is
// asked, and a second browser must not disagree.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { wipeCache } from "../src/core/localCache.js";

import { mountWatchSetting, watchSettingPanelHtml, watchesAgentFiledTasks } from "../src/core/watchSetting.js";

const host = () => document.querySelector("#panels");
const box = () => host().querySelector("#watchagenttasks");
const error = () => host().querySelector("#watchagenttaskserr").textContent;

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = `<div id="panels">${watchSettingPanelHtml()}</div>`;
});

describe("what the bridge's answer means", () => {
  it("is off until the machine explicitly saves on", () => {
    expect(watchesAgentFiledTasks({})).toBe(false);
    expect(watchesAgentFiledTasks(null)).toBe(false);
    expect(watchesAgentFiledTasks(undefined)).toBe(false);
    expect(watchesAgentFiledTasks({ watch_agent_filed_tasks: true })).toBe(true);
  });

  it("is off when the machine says off", () => {
    expect(watchesAgentFiledTasks({ watch_agent_filed_tasks: false })).toBe(false);
  });
});

describe("the switch", () => {
  it("shows what the machine holds, not what the page assumed", async () => {
    const callRpc = vi.fn(async () => ({ watch_agent_filed_tasks: false }));
    await mountWatchSetting(host(), { callRpc });
    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(box().checked).toBe(false);
    expect(box().disabled).toBe(false);
  });

  it("saves the change and repaints from the bridge's own answer", async () => {
    const answers = { "settings.get": { watch_agent_filed_tasks: true } };
    const callRpc = vi.fn(async (method, params) => {
      if (method === "settings.set") return { watch_agent_filed_tasks: params.watch_agent_filed_tasks };
      return answers[method];
    });
    const onSaved = vi.fn();
    await mountWatchSetting(host(), { callRpc, onSaved });

    box().checked = false;
    await box().onchange();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { watch_agent_filed_tasks: false });
    expect(box().checked).toBe(false);
    expect(onSaved).toHaveBeenCalled();
  });

  // The control must never show a choice the machine did not confirm.
  it("puts the switch back and says so when the save is refused", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.set") throw new Error("device is away");
      return { watch_agent_filed_tasks: true };
    });
    await mountWatchSetting(host(), { callRpc });

    box().checked = false;
    await box().onchange();

    expect(box().checked).toBe(true);
    expect(error()).toContain("device is away");
  });

  it("says so when the machine cannot be read at all, and offers no switch", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("no session");
    });
    await mountWatchSetting(host(), { callRpc });
    expect(error()).toContain("no session");
    expect(box().disabled).toBe(true);
  });
});
