// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { mountTriageSetting, triageEnabledOf, triageSettingPanelHtml } from "../src/core/triageSetting.js";

const flush = () => new Promise((done) => setTimeout(done, 0));
const mount = async (callRpc) => {
  document.body.innerHTML = triageSettingPanelHtml();
  await mountTriageSetting(document.body, { callRpc });
  return document.querySelector('[data-triage-setting="control"]');
};

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the review-prioritization account setting", () => {
  it("accepts only an explicit bridge boolean", () => {
    expect(triageEnabledOf({ triage_enabled: false })).toBe(false);
    expect(triageEnabledOf({ triage_enabled: true })).toBe(true);
    expect(() => triageEnabledOf({})).toThrow(/unavailable/i);
  });

  it("paints the default-off setting and enables the control", async () => {
    const control = await mount(vi.fn(async () => ({ triage_enabled: false })));
    expect(control.checked).toBe(false);
    expect(control.disabled).toBe(false);
  });

  it("saves immediately and repaints from the bridge response", async () => {
    const callRpc = vi.fn(async (method) =>
      method === "settings.get" ? { triage_enabled: false } : { triage_enabled: true },
    );
    const control = await mount(callRpc);
    control.checked = true;
    control.dispatchEvent(new Event("change"));
    await flush();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { triage_enabled: true });
    expect(control.checked).toBe(true);
    expect(document.querySelector('[data-triage-setting="saved"]').textContent).toContain("Saved");
  });

  it("restores the confirmed value when the bridge refuses the save", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { triage_enabled: false };
      throw new Error("cannot write settings");
    });
    const control = await mount(callRpc);
    control.checked = true;
    control.dispatchEvent(new Event("change"));
    await flush();

    expect(control.checked).toBe(false);
    expect(control.disabled).toBe(false);
    expect(document.querySelector('[data-triage-setting="error"]').textContent).toBe("cannot write settings");
  });

  it("re-reads after a lost save response and shows what the bridge persisted", async () => {
    let reads = 0;
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return { triage_enabled: reads++ > 0 };
      throw new Error("response was lost");
    });
    const control = await mount(callRpc);
    control.checked = true;
    control.dispatchEvent(new Event("change"));
    await flush();

    expect(reads).toBe(2);
    expect(control.checked).toBe(true);
    expect(control.disabled).toBe(false);
    expect(document.querySelector('[data-triage-setting="error"]').textContent).toBe("response was lost");
  });

  it("fails closed on an older bridge response", async () => {
    const control = await mount(vi.fn(async () => ({})));
    expect(control.checked).toBe(false);
    expect(control.disabled).toBe(true);
    expect(document.querySelector('[data-triage-setting="error"]').textContent).toMatch(/unavailable/i);
  });
});
