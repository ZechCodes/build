// @vitest-environment jsdom
// Starting an agent in a checkout from the Agent tab's provider cards.
//
// Starting one is a mutating act on that directory, so it adopts — and the card
// the human pressed is the answer to "which harness runs here", which has to
// reach BOTH the adopt (the run is minted on that provider) and the start (a
// worktree adopted earlier still switches to it).

import { describe, it, expect, vi } from "vitest";
import { startAdoptedAgent, createAdoptingCall } from "../src/core/adoption.js";

const fakeAdopting = () => ({
  setAdoptParams: vi.fn(),
  runCall: vi.fn(async () => ({ ok: true })),
});

describe("startAdoptedAgent", () => {
  it("seeds the adopt with the picked provider and names it on the start", async () => {
    const adopting = fakeAdopting();
    await startAdoptedAgent(adopting, "codex", null);
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "codex" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "codex" });
  });

  // No UI path omits the provider any more (every picker control names one),
  // but the helper's contract still covers it: the run already holds a choice.
  it("starts on the run's own provider when none was picked", async () => {
    const adopting = fakeAdopting();
    await startAdoptedAgent(adopting, undefined, null);
    expect(adopting.setAdoptParams).not.toHaveBeenCalled();
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", {});
  });

  // The new-worktree sheet already asked which agent works here, so a restart
  // (or a start on a worktree Build just minted) honors that answer.
  it("falls back to the answer the new-worktree sheet already got", async () => {
    const adopting = fakeAdopting();
    await startAdoptedAgent(adopting, undefined, "claude");
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "claude" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "claude" });
  });

  // A press is a later, more explicit answer than the sheet's.
  it("lets the pressed card override the sheet's answer", async () => {
    const adopting = fakeAdopting();
    await startAdoptedAgent(adopting, "codex", "claude");
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "codex" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "codex" });
  });

  // Through a real adopter: the pressed card mints the run that owns the
  // checkout, then names itself on the start.
  it("mints the run on the provider the card named, then starts on it", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-main" } : { ok: true }));
    const adopting = createAdoptingCall(call, "proj-1", "wt-1");

    await startAdoptedAgent(adopting, "claude");

    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", { project_id: "proj-1", worktree_id: "wt-1", provider: "claude" });
    expect(call).toHaveBeenNthCalledWith(2, "agent.start", { run_id: "run-main", provider: "claude" });
  });
});
