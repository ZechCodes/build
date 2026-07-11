import { describe, it, expect, vi } from "vitest";
import { createAdoptingCall } from "../src/core/adoption.js";

describe("createAdoptingCall", () => {
  it("adopts on the first taskCall, then issues the method with the minted task_id", async () => {
    const call = vi.fn(async (method) => {
      if (method === "task.adopt") return { task_id: "task-7", state: "review", adopted: true };
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "proj-1", "wt-abc");
    expect(adopting.adoptedTaskId()).toBe(null);

    const res = await adopting.taskCall("task.request_changes", { comments: "fix it" });

    expect(call).toHaveBeenNthCalledWith(1, "task.adopt", { project_id: "proj-1", worktree_id: "wt-abc" });
    expect(call).toHaveBeenNthCalledWith(2, "task.request_changes", { task_id: "task-7", comments: "fix it" });
    expect(res).toEqual({ ok: true });
    expect(adopting.adoptedTaskId()).toBe("task-7");
  });

  it("adopts exactly once across multiple taskCalls", async () => {
    const call = vi.fn(async (method) => (method === "task.adopt" ? { task_id: "task-9" } : { ok: true }));
    const adopting = createAdoptingCall(call, "p", "wt-1");
    await adopting.taskCall("task.git_action", { action: "merge", cleanup: "keep" });
    await adopting.taskCall("task.abandon", {});
    const adoptCalls = call.mock.calls.filter((c) => c[0] === "task.adopt");
    expect(adoptCalls).toHaveLength(1);
    expect(call).toHaveBeenNthCalledWith(3, "task.abandon", { task_id: "task-9" });
  });

  it("propagates an adopt rejection and stays un-adopted so a retry re-adopts", async () => {
    let attempts = 0;
    const call = vi.fn(async (method) => {
      if (method === "task.adopt") {
        attempts += 1;
        if (attempts === 1) throw new Error("boom");
        return { task_id: "task-3" };
      }
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "p", "wt-x");

    await expect(adopting.taskCall("task.abandon", {})).rejects.toThrow("boom");
    expect(adopting.adoptedTaskId()).toBe(null);

    const res = await adopting.taskCall("task.abandon", {});
    expect(res).toEqual({ ok: true });
    expect(adopting.adoptedTaskId()).toBe("task-3");
    expect(attempts).toBe(2);
  });

  it("merges params without a params object supplied", async () => {
    const call = vi.fn(async (method) => (method === "task.adopt" ? { task_id: "t" } : { ok: true }));
    const adopting = createAdoptingCall(call, "p", "w");
    await adopting.taskCall("task.abandon");
    expect(call).toHaveBeenNthCalledWith(2, "task.abandon", { task_id: "t" });
  });
});
