// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({ projects: [], capability: true, greeting: null }));
vi.mock("../src/core/localCache.js", () => ({ readCached: vi.fn(async () => ({ value: state.projects })) }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => ({ term: { projectSources: state.capability } }) }));
vi.mock("../src/core/deviceContexts.js", () => ({ whenGreeted: async (_context, dispatch) => {
  await state.greeting;
  return { sent: dispatch(() => true), stands: () => true };
} }));
const { projectTerminalRpc } = await import("../src/core/projectTerminalRpc.js");
const scope = { project_id: "p1", source_id: "s1", source_path: "/original" };
const context = { deviceId: "d1" };
beforeEach(() => {
  state.projects = [{ id: "p1", sources: [{ id: "s1", path: "/original" }] }];
  state.capability = true;
  state.greeting = null;
});
describe("project terminal dispatch", () => {
  it.each(["term.list", "term.create", "term.attach", "term.close"])("sends %s with exact original source assertions", async (method) => {
    const call = vi.fn(async () => ({ term_id: "t1" }));
    await expect(projectTerminalRpc(context, method, { ...scope, term_id: "t1" }, call)).resolves.toEqual({ term_id: "t1" });
    expect(call).toHaveBeenCalledWith(method, { ...scope, term_id: "t1" });
  });
  it("refuses an older bridge and never sends project source selectors", async () => {
    state.capability = false;
    const call = vi.fn();
    await expect(projectTerminalRpc(context, "term.create", scope, call)).rejects.toThrow(/Update/);
    expect(call).not.toHaveBeenCalled();
  });
  it.each([{ source_path: "/moved" }, { source_id: "other" }, { project_id: "other" }])("refuses absent or changed membership %j", async (over) => {
    const call = vi.fn();
    await expect(projectTerminalRpc(context, "term.attach", { ...scope, ...over }, call)).rejects.toThrow(/moved or was removed/);
    expect(call).not.toHaveBeenCalled();
  });
  it("checks membership after a pending greeting settles", async () => {
    let greet;
    state.greeting = new Promise((resolve) => { greet = resolve; });
    const call = vi.fn();
    const request = projectTerminalRpc(context, "term.list", scope, call);
    state.projects[0].sources[0].path = "/replacement";
    greet();
    await expect(request).rejects.toThrow(/moved or was removed/);
    expect(call).not.toHaveBeenCalled();
  });
  it("uses this device's cache scope", async () => {
    const { readCached } = await import("../src/core/localCache.js");
    await projectTerminalRpc(context, "term.list", scope, vi.fn(async () => ({})));
    expect(readCached).toHaveBeenCalledWith({ deviceId: "d1", entityId: "", kind: "projects" });
  });
});
