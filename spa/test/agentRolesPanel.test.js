/** @vitest-environment jsdom */
// The models-and-roles panel against a fake bridge.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { wipeCache } from "../src/core/localCache.js";
import { agentRolesPanelHtml, mountAgentRoles } from "../src/core/agentRolesPanel.js";

const DECLARED = [
  { model: "claude-fable-5-1", roles: ["planner", "reviewer"], capability: "generalist" },
  { model: "claude-opus-5", roles: ["planner", "reviewer", "implementer"], capability: "scoped" },
];

const stand = async ({ refuse = false } = {}) => {
  document.body.innerHTML = agentRolesPanelHtml();
  const calls = [];
  let held = DECLARED;
  const callRpc = vi.fn(async (method, params) => {
    calls.push([method, params]);
    if (method === "settings.get") return { role_models: held };
    if (method === "settings.set") {
      if (refuse) throw new Error("the device said no");
      held = params.role_models;
      return { role_models: held };
    }
    throw new Error(`unexpected ${method}`);
  });
  await mountAgentRoles(document.body, { callRpc });
  return { calls, sent: () => calls.filter(([method]) => method === "settings.set").at(-1)?.[1] };
};

const rows = () => [...document.querySelectorAll("[data-aroles-rows] tr")];

beforeEach(async () => { await wipeCache(); });

describe("the panel", () => {
  it("draws a row per declared model, in the device's own order", async () => {
    await stand();
    expect(rows()).toHaveLength(2);
    expect(rows()[0].querySelector("th").textContent).toContain("claude-fable-5-1");
    expect(rows()[0].querySelector('[data-role="reviewer"]').checked).toBe(true);
    expect(rows()[0].querySelector('[data-role="implementer"]').checked).toBe(false);
    expect(rows()[0].querySelector("[data-capability]").value).toBe("generalist");
  });

  it("toggles a role and sends the whole list back", async () => {
    const { sent } = await stand();
    const box = rows()[0].querySelector('[data-role="implementer"]');
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => expect(document.querySelector("#arolessaved").textContent).toBe("Saved."));

    expect(sent().role_models[0].roles).toEqual(["planner", "implementer", "reviewer"]);
    expect(sent().role_models[1]).toEqual(DECLARED[1]);
    await vi.waitFor(() => expect(document.querySelector("#arolessaved").textContent).toBe("Saved."));
  });

  // Moving a row IS an edit: it changes which model fills a shared role.
  it("moves a row, which changes what fills a shared role", async () => {
    const { sent } = await stand();
    rows()[1].querySelector('[data-move="-1"]').click();
    await vi.waitFor(() => expect(rows()[0].querySelector("th").textContent).toContain("claude-opus-5"));

    expect(sent().role_models.map((row) => row.model)).toEqual([
      "claude-opus-5",
      "claude-fable-5-1",
    ]);
    // And the first row can no longer be moved up.
    expect(rows()[0].querySelector('[data-move="-1"]').disabled).toBe(true);
  });

  it("adds a model with no roles, and removes a row", async () => {
    const { sent } = await stand();
    const form = document.querySelector("[data-aroles-add]");
    form.elements.model.value = "claude-haiku-4-5";
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(rows()).toHaveLength(3));
    expect(sent().role_models.at(-1)).toEqual({
      model: "claude-haiku-4-5",
      roles: [],
      capability: "scoped",
    });

    rows()[0].querySelector("[data-remove]").click();
    await vi.waitFor(() => expect(rows()).toHaveLength(2));
    expect(sent().role_models.map((row) => row.model)).not.toContain("claude-fable-5-1");
  });

  // A refused save must leave the panel showing what the device holds.
  it("says why a refused save failed and keeps showing the device's list", async () => {
    await stand({ refuse: true });
    const box = rows()[0].querySelector('[data-role="executor"]');
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => expect(document.querySelector("#aroleserr").textContent).toContain("the device said no"));

    expect(document.querySelector("#aroleserr").textContent).toContain("the device said no");
    expect(rows()[0].querySelector('[data-role="executor"]').checked).toBe(false);
  });
});
