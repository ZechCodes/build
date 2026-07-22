import { describe, it, expect, vi } from "vitest";
import { createAdoptingCall } from "../src/core/adoption.js";

describe("createAdoptingCall", () => {
  it("adopts on the first runCall, then issues the method with the minted run_id", async () => {
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") return { run_id: "run-7", state: "review", adopted: true };
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "proj-1", "wt-abc");
    expect(adopting.adoptedRunId()).toBe(null);

    const res = await adopting.runCall("run.request_changes", { comments: "fix it" });

    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", { project_id: "proj-1", worktree_id: "wt-abc" });
    expect(call).toHaveBeenNthCalledWith(2, "run.request_changes", { run_id: "run-7", comments: "fix it" });
    expect(res).toEqual({ ok: true });
    expect(adopting.adoptedRunId()).toBe("run-7");
  });

  it("adopts exactly once across multiple runCalls", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-9" } : { ok: true }));
    const adopting = createAdoptingCall(call, "p", "wt-1");
    await adopting.runCall("run.git_action", { action: "merge", cleanup: "keep" });
    await adopting.runCall("run.abandon", {});
    const adoptCalls = call.mock.calls.filter((c) => c[0] === "run.adopt");
    expect(adoptCalls).toHaveLength(1);
    expect(call).toHaveBeenNthCalledWith(3, "run.abandon", { run_id: "run-9" });
  });

  it("propagates an adopt rejection and stays un-adopted so a retry re-adopts", async () => {
    let attempts = 0;
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") {
        attempts += 1;
        if (attempts === 1) throw new Error("boom");
        return { run_id: "run-3" };
      }
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "p", "wt-x");

    await expect(adopting.runCall("run.abandon", {})).rejects.toThrow("boom");
    expect(adopting.adoptedRunId()).toBe(null);

    const res = await adopting.runCall("run.abandon", {});
    expect(res).toEqual({ ok: true });
    expect(adopting.adoptedRunId()).toBe("run-3");
    expect(attempts).toBe(2);
  });

  it("merges params without a params object supplied", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "r" } : { ok: true }));
    const adopting = createAdoptingCall(call, "p", "w");
    await adopting.runCall("run.abandon");
    expect(call).toHaveBeenNthCalledWith(2, "run.abandon", { run_id: "r" });
  });

  it("includes the selected agent choice when adoption first mints the run", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "r" } : { ok: true }));
    const adopting = createAdoptingCall(call, "p", "w");
    adopting.setAdoptParams({ provider: "codex", model: "gpt-5.6-sol", effort: "high" });
    await adopting.runCall("run.request_changes", { comments: "fix it" });
    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", {
      project_id: "p",
      worktree_id: "w",
      provider: "codex",
      model: "gpt-5.6-sol",
      effort: "high",
    });
  });
});
