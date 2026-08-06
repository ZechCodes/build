import { describe, it, expect, vi } from "vitest";
import { createAdoptingCall, createPrimaryAdoptingCall } from "../src/core/adoption.js";

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

// The primary checkout adopts through the same latch: only the scope it names
// differs (the repo root has no worktree id), and the run it mints owns the
// repository the user works in directly.
describe("createPrimaryAdoptingCall", () => {
  it("adopts the project's primary checkout, then routes through the minted run", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-main" } : { ok: true }));
    const adopting = createPrimaryAdoptingCall(call, "proj-1");

    await adopting.runCall("agent.start", { provider: "claude" });

    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", { project_id: "proj-1", primary: true });
    expect(call).toHaveBeenNthCalledWith(2, "agent.start", { run_id: "run-main", provider: "claude" });
    expect(adopting.adoptedRunId()).toBe("run-main");
  });

  it("mints the run on the picked provider", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-main" } : { ok: true }));
    const adopting = createPrimaryAdoptingCall(call, "proj-1");
    adopting.setAdoptParams({ provider: "codex" });
    await adopting.adopt();
    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", { project_id: "proj-1", primary: true, provider: "codex" });
  });

  // Reconnect after a reload: the run that already owns the checkout is learned
  // read-only, so nothing is minted and every later call routes through it.
  it("binds to a run that already owns the checkout without adopting", async () => {
    const call = vi.fn(async () => ({ ok: true }));
    const adopting = createPrimaryAdoptingCall(call, "proj-1");
    adopting.seedAdoptedRun("run-existing");
    expect(adopting.adoptedRunId()).toBe("run-existing");

    await adopting.runCall("agent.start", {});

    expect(call.mock.calls.filter((c) => c[0] === "run.adopt")).toHaveLength(0);
    expect(call).toHaveBeenCalledWith("agent.start", { run_id: "run-existing" });
  });

  it("ignores a seed once the checkout has been adopted here", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-mine" } : { ok: true }));
    const adopting = createPrimaryAdoptingCall(call, "proj-1");
    await adopting.adopt();
    adopting.seedAdoptedRun("run-other");
    expect(adopting.adoptedRunId()).toBe("run-mine");
  });

  // A terminal run has let go of the checkout (the bridge stops reporting it as
  // the owner), so the next action adopts a fresh owner rather than posting into
  // a run that no longer works here.
  it("releases the checkout so the next action adopts again", async () => {
    let minted = 0;
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: `run-${++minted}` } : { ok: true }));
    const adopting = createPrimaryAdoptingCall(call, "proj-1");
    await adopting.adopt();
    adopting.releaseAdoptedRun();
    expect(adopting.adoptedRunId()).toBe(null);

    await adopting.adopt();

    expect(adopting.adoptedRunId()).toBe("run-2");
  });
});
