/** @vitest-environment jsdom */
// The one switch watching needs (#65): whether an issue an agent files is one
// the user hears about.
//
// It is the DEVICE's, beside the other bridge settings, because the bridge is
// what decides whether to watch at the moment an agent files — no browser is
// asked, and a second browser must not disagree.

import { describe, it, expect, vi, beforeEach } from "vitest";

import { mountWatchSetting, watchSettingPanelHtml, watchesAgentFiledIssues } from "../src/core/watchSetting.js";

const host = () => document.querySelector("#panels");
const box = () => host().querySelector("#watchagentissues");
const error = () => host().querySelector("#watchagentissueserr").textContent;

beforeEach(() => {
  document.body.innerHTML = `<div id="panels">${watchSettingPanelHtml()}</div>`;
});

describe("what the bridge's answer means", () => {
  // Default ON: an agent filing an issue for you is the case the feature is
  // for, so it works before anybody visits Settings.
  it("is on unless the machine says otherwise", () => {
    expect(watchesAgentFiledIssues({})).toBe(true);
    expect(watchesAgentFiledIssues(null)).toBe(true);
    expect(watchesAgentFiledIssues(undefined)).toBe(true);
    expect(watchesAgentFiledIssues({ watch_agent_filed_issues: true })).toBe(true);
  });

  it("is off only when the machine says off", () => {
    expect(watchesAgentFiledIssues({ watch_agent_filed_issues: false })).toBe(false);
  });
});

describe("the switch", () => {
  it("shows what the machine holds, not what the page assumed", async () => {
    const callRpc = vi.fn(async () => ({ watch_agent_filed_issues: false }));
    await mountWatchSetting(host(), { callRpc });
    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(box().checked).toBe(false);
    expect(box().disabled).toBe(false);
  });

  it("saves the change and repaints from the bridge's own answer", async () => {
    const answers = { "settings.get": { watch_agent_filed_issues: true } };
    const callRpc = vi.fn(async (method, params) => {
      if (method === "settings.set") return { watch_agent_filed_issues: params.watch_agent_filed_issues };
      return answers[method];
    });
    const onSaved = vi.fn();
    await mountWatchSetting(host(), { callRpc, onSaved });

    box().checked = false;
    await box().onchange();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { watch_agent_filed_issues: false });
    expect(box().checked).toBe(false);
    expect(onSaved).toHaveBeenCalled();
  });

  // The control must never show a choice the machine did not confirm.
  it("puts the switch back and says so when the save is refused", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.set") throw new Error("device is away");
      return { watch_agent_filed_issues: true };
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
