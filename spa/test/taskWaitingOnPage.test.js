/** @vitest-environment jsdom */
import { beforeEach, describe, expect, it, vi } from "vitest";

const shared = vi.hoisted(() => ({ supported: true, tasks: [], pages: null, subscribers: [], add: vi.fn(), remove: vi.fn(), blocks: vi.fn(), retry: vi.fn() }));
vi.mock("../src/core/taskWaitingOnActions.js", () => ({ createTaskWaitingOnActions: () => shared }));
vi.mock("../src/core/taskWaitingOnSupport.js", () => ({ readTaskWaitingOnSupport: async () => shared.supported, taskWaitingOnSupportAddress: () => ({ kind: "support" }) }));
vi.mock("../src/core/trackerCache.js", () => ({ readTasksRecord: async () => ({ tasks: shared.tasks }), tasksAddress: () => ({ kind: "tasks" }) }));
vi.mock("../src/core/taskWaitingOnCache.js", () => ({ readTaskWaitingOnBlocks: async () => shared.pages, taskWaitingOnBlocksAddress: () => ({ kind: "blocks" }), taskWaitingOnGenerationAddress: () => ({ kind: "generation" }) }));
vi.mock("../src/core/localCache.js", () => ({ subscribeCache: (address, handler) => { shared.subscribers.push({ address, handler }); return () => {}; } }));
import { mountTaskWaitingOn } from "../src/core/taskWaitingOnPage.js";

const edge = (id, resolved = false) => ({ task_id: id, number: 2, title: "Prerequisite", resolved, resolved_by: resolved ? "done" : null });
const fixture = (extra = {}) => ({ id: "task", project_id: "p", dependencies: { waiting_on: [edge("b")], blocks: [edge("dependent")], blocks_count: 2, blocks_next_cursor: "next" }, ...extra });
const flush = async () => { for (let index = 0; index < 4; index++) await Promise.resolve(); };
let host, view;
const find = (selector) => host.querySelector(selector);
const mount = async (task = fixture()) => { host = document.createElement("div"); document.body.replaceChildren(host); view = mountTaskWaitingOn(host, { deviceId: "d", projectId: "p", taskId: "task", call: vi.fn() }); view.update(task); await flush(); };

beforeEach(() => {
  shared.supported = true; shared.pages = null; shared.subscribers = [];
  shared.tasks = [{ id: "c", project_id: "p", number: 3, title: "Compile" }, { id: "foreign", project_id: "q", number: 4, title: "Other" }];
  for (const method of ["add", "remove", "blocks", "retry"]) shared[method].mockReset().mockResolvedValue({ task: fixture() });
});

describe("kept Waiting on editor", () => {
  it("renders cached links and keeps picker nodes and text across task repaints and refusals", async () => {
    await mount();
    const input = find("[data-waiting-search]");
    const select = find("[data-waiting-choice]");
    input.value = "Comp"; input.dispatchEvent(new Event("input"));
    select.value = "c"; select.dispatchEvent(new Event("change"));
    shared.add.mockRejectedValue({ code: "cycle", message: "This would create a cycle" });
    find("[data-waiting-picker]").dispatchEvent(new Event("submit", { cancelable: true })); await flush();
    expect(shared.add).toHaveBeenCalledWith(["c"]);
    expect(input.value).toBe("Comp"); expect(select.value).toBe("c");
    view.update(fixture({ title: "Changed elsewhere" }));
    expect(find("[data-waiting-search]")).toBe(input);
    expect(find("[data-waiting-choice]")).toBe(select);
    expect(select.value).toBe("c");
    expect(find("[data-waiting-error]").textContent).toContain("cycle");
    expect(host.textContent).not.toContain("Other");
    view.dispose();
  });
  it("confirms the last unresolved edge consequence and retains confirmation on stale refusal", async () => {
    await mount(fixture({ deferred_assignment: { id: "saved", state: "waiting" } }));
    find('[data-waiting-remove="b"]').click();
    expect(find("[data-waiting-confirm]").hidden).toBe(false);
    expect(find("[data-waiting-consequence]").textContent).toContain("starts the saved assignment");
    expect(shared.remove).not.toHaveBeenCalled();
    shared.remove.mockRejectedValue(new Error("Task changed; reload before editing"));
    find("[data-waiting-confirm-remove]").click(); await flush();
    expect(shared.remove).toHaveBeenCalledWith(["b"]);
    expect(find("[data-waiting-confirm]").hidden).toBe(false);
    expect(find("[data-waiting-error]").textContent).toContain("changed");
    view.dispose();
  });
  it("pages reverse edges through the shared action and paints its cache announcement", async () => {
    await mount();
    find("[data-blocks-more]").click(); await flush();
    expect(shared.blocks).toHaveBeenCalledWith({ cursor: "next" });
    shared.pages = { blocks: [edge("dependent"), edge("more")], next_cursor: null };
    await shared.subscribers.find((item) => item.address.kind === "blocks").handler();
    expect(find("[data-blocks-list]").querySelectorAll("a")).toHaveLength(2);
    expect(find("[data-blocks-more]").hidden).toBe(true);
    view.dispose();
  });
  it("drops a paged reverse chain when fresh task dependencies invalidate its cache generation", async () => {
    shared.pages = { blocks: [edge("removed-dependent")], next_cursor: null };
    await mount();
    expect(find("[data-blocks-list]").querySelectorAll("a")).toHaveLength(1);
    shared.pages = null;
    view.update(fixture({ dependencies: { waiting_on: [], blocks: [], blocks_count: 0 } }));
    await flush();
    expect(find("[data-blocks-list]").querySelectorAll("a")).toHaveLength(0);
    view.dispose();
  });

  it("drops off-preview pages on a project generation announcement while keeping the preview cursor", async () => {
    shared.pages = { blocks: [edge("dependent"), edge("off-preview")], next_cursor: "stale-tail" };
    await mount();
    expect(find("[data-blocks-list]").querySelectorAll("a")).toHaveLength(2);
    shared.pages = null;
    await shared.subscribers.find((item) => item.address.kind === "generation").handler();
    expect(find("[data-blocks-list]").querySelectorAll("a")).toHaveLength(1);
    find("[data-blocks-more]").click(); await flush();
    expect(shared.blocks).toHaveBeenCalledWith({ cursor: "next" });
    view.dispose();
  });

  it("retries a failed intent by its exact saved id", async () => {
    await mount(fixture({ deferred_assignment: { id: "saved-failed-id", state: "failed", error: "workspace unavailable" } }));
    find("[data-waiting-retry]").click(); await flush();
    expect(shared.retry).toHaveBeenCalledWith("saved-failed-id");
    expect(find("[data-waiting-delivery-error]").textContent).toBe("workspace unavailable");
    view.dispose();
  });
  it("draws no new controls for a cached unsupported bridge", async () => {
    shared.supported = false; await mount();
    expect(host.hidden).toBe(true); expect(shared.add).not.toHaveBeenCalled(); view.dispose();
  });
});
