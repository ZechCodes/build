import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import {
  ADOPT_REASK_LIMIT,
  ADOPT_REASK_MS,
  createAdopters,
  createAdoptingCall,
  createPrimaryAdoptingCall,
} from "../src/core/adoption.js";

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

// The daemon answers an adopt after its git — a scan, a checkpoint commit, a
// scaffold — so the reply can outlive the browser's timer while the adoption
// succeeds behind it. That is not a refusal, and the next action must not read
// it as one: the checkout is being claimed, and the one adoption stays in
// flight until the daemon names its run.
describe("an adoption the browser stopped waiting for", () => {
  const timedOut = () => {
    const error = new Error("run.adopt timed out");
    error.timedOut = true;
    return error;
  };

  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("keeps the one adoption alive and asks again, so a second action sends no second adopt and raises nothing", async () => {
    let asks = 0;
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") {
        asks += 1;
        if (asks === 1) throw timedOut();
        return { run_id: "run-late", adopted: true };
      }
      return { ok: true };
    });
    const adopting = createPrimaryAdoptingCall(call, "proj-1");

    const first = adopting.runCall("agent.start", {});
    await vi.advanceTimersByTimeAsync(0);
    const second = adopting.runCall("run.request_changes", { comments: "fix it" });
    await vi.advanceTimersByTimeAsync(0);
    expect(asks).toBe(1);
    expect(adopting.adoptedRunId()).toBe(null);

    await vi.advanceTimersByTimeAsync(ADOPT_REASK_MS);
    await expect(first).resolves.toEqual({ ok: true });
    await expect(second).resolves.toEqual({ ok: true });
    expect(asks).toBe(2);
    expect(call).toHaveBeenCalledWith("agent.start", { run_id: "run-late" });
    expect(call).toHaveBeenCalledWith("run.request_changes", { run_id: "run-late", comments: "fix it" });
  });

  // The daemon's own word for the same thing: an adopt that arrives while the
  // checkout is being claimed is told so, and handed no run — none is durable
  // yet. It reads exactly as the timer does.
  it("reads a reply that names no run as the adoption still running", async () => {
    let asks = 0;
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") {
        asks += 1;
        return asks === 1 ? { adopting: true } : { run_id: "run-owner" };
      }
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "p", "wt-x");

    const action = adopting.runCall("run.abandon", {});
    await vi.advanceTimersByTimeAsync(ADOPT_REASK_MS);

    await expect(action).resolves.toEqual({ ok: true });
    expect(adopting.adoptedRunId()).toBe("run-owner");
  });

  it("raises a real refusal on the re-ask and lets the next action adopt afresh", async () => {
    let asks = 0;
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") {
        asks += 1;
        if (asks === 1) throw timedOut();
        if (asks === 2) throw new Error("cannot adopt: HEAD is detached");
        return { run_id: "run-3" };
      }
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "p", "wt-x");

    const action = adopting.runCall("run.abandon", {});
    action.catch(() => {});
    await vi.advanceTimersByTimeAsync(ADOPT_REASK_MS);
    await expect(action).rejects.toThrow("HEAD is detached");
    expect(adopting.adoptedRunId()).toBe(null);

    await expect(adopting.runCall("run.abandon", {})).resolves.toEqual({ ok: true });
    expect(asks).toBe(3);
  });

  it("stops asking after the bound and says so, rather than polling forever", async () => {
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") throw timedOut();
      return { ok: true };
    });
    const adopting = createPrimaryAdoptingCall(call, "proj-1");

    const action = adopting.runCall("agent.start", {});
    action.catch(() => {});
    await vi.advanceTimersByTimeAsync(ADOPT_REASK_MS * (ADOPT_REASK_LIMIT + 1));

    await expect(action).rejects.toThrow("never answered");
    expect(call.mock.calls.filter((c) => c[0] === "run.adopt")).toHaveLength(ADOPT_REASK_LIMIT + 1);
    expect(adopting.adoptedRunId()).toBe(null);
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

// A branch view has two surfaces that can each mutate first — the agent rail
// and the Changes review — and each one adopting on its own would ask the
// bridge for two owners of the same checkout.
describe("createAdopters", () => {
  const scopeCall = () => vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-7" } : { ok: true }));

  it("hands every surface on one checkout the same adopter", async () => {
    const call = scopeCall();
    const adopterFor = createAdopters(call);
    const scope = { project_id: "p1", worktree_id: "wt-3" };

    const rail = adopterFor(scope);
    const review = adopterFor({ ...scope });
    expect(review).toBe(rail);

    await Promise.all([rail.adopt(), review.adopt()]);
    expect(call.mock.calls.filter((c) => c[0] === "run.adopt")).toHaveLength(1);
  });

  it("keeps two checkouts apart", () => {
    const adopterFor = createAdopters(scopeCall());
    const one = adopterFor({ project_id: "p1", worktree_id: "wt-3" });
    const other = adopterFor({ project_id: "p1", worktree_id: "wt-4" });
    expect(other).not.toBe(one);
  });

  it("adopts the primary checkout when the scope names no worktree", async () => {
    const call = scopeCall();
    const adopterFor = createAdopters(call);
    await adopterFor({ project_id: "p1" }).adopt();
    expect(call).toHaveBeenCalledWith("run.adopt", { project_id: "p1", primary: true });
  });

  it("has no adopter for a checkout Build already owns, or for no checkout at all", () => {
    const adopterFor = createAdopters(scopeCall());
    expect(adopterFor({ run_id: "run-3" })).toBe(null);
    expect(adopterFor(null)).toBe(null);
  });
});
