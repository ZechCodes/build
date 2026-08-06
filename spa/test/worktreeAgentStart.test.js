// @vitest-environment jsdom
// Starting an agent in an external worktree from the Agent tab's provider cards.
//
// Starting one is a mutating act on that directory, so it adopts — and the card
// the human pressed is the answer to "which harness runs here", which has to
// reach BOTH the adopt (the run is minted on that provider) and the start (a
// worktree adopted earlier still switches to it).

import { describe, it, expect, vi } from "vitest";
import { startWorktreeAgent } from "../src/views/worktree.js";

const fakeAdopting = () => ({
  setAdoptParams: vi.fn(),
  runCall: vi.fn(async () => ({ ok: true })),
});

describe("startWorktreeAgent", () => {
  it("seeds the adopt with the picked provider and names it on the start", async () => {
    const adopting = fakeAdopting();
    await startWorktreeAgent(adopting, "codex", null);
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "codex" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "codex" });
  });

  // The Restart button names no provider: the run already holds one.
  it("starts on the run's own provider when none was picked", async () => {
    const adopting = fakeAdopting();
    await startWorktreeAgent(adopting, undefined, null);
    expect(adopting.setAdoptParams).not.toHaveBeenCalled();
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", {});
  });

  // The new-worktree sheet already asked which agent works here, so a restart
  // (or a start on a worktree Build just minted) honors that answer.
  it("falls back to the answer the new-worktree sheet already got", async () => {
    const adopting = fakeAdopting();
    await startWorktreeAgent(adopting, undefined, "claude");
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "claude" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "claude" });
  });

  // A press is a later, more explicit answer than the sheet's.
  it("lets the pressed card override the sheet's answer", async () => {
    const adopting = fakeAdopting();
    await startWorktreeAgent(adopting, "codex", "claude");
    expect(adopting.setAdoptParams).toHaveBeenCalledWith({ provider: "codex" });
    expect(adopting.runCall).toHaveBeenCalledWith("agent.start", { provider: "codex" });
  });
});
