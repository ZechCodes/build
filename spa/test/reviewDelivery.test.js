// Comments on a worktree's changes reach that worktree's ONE agent.
//
// The client's half of that path, with the real collaborators the Changes tab
// uses: the sheet's chosen agent (the one-shot marker) becomes the run's
// provider, the reviewer's comments become durable thread messages, and a
// single run.request_changes carries them. On a worktree Build has not adopted,
// the first send adopts first — a run is what gives the agent an owner and its
// MCP config, so it must exist before a harness can — and everything after that
// rides the same run id, which is to say the same agent tab.

import { describe, it, expect, vi } from "vitest";
import { createAdoptingCall } from "../src/core/adoption.js";
import { markNewWorktree, takeNewWorktreeMark } from "../src/core/newWorktree.js";
import { diffThreadMessages } from "../src/core/notes.js";
import { catalogForProvider, modelParams, normalizeModelCatalog } from "../src/core/modelPicker.js";

const memoryStorage = () => {
  const map = new Map();
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, v),
    removeItem: (k) => map.delete(k),
  };
};

const CATALOG = normalizeModelCatalog({
  default_provider: "claude",
  providers: [
    { id: "claude", label: "Claude Code", models: [], efforts: [] },
    { id: "codex", label: "Codex", models: [], efforts: [] },
  ],
});

const COMMENTS = [{ id: 1, file: "src/app.rs", lnA: 10, lnB: 12, snippet: "let x = 1;", comment: "name this" }];

/** What the worktree Changes tab does on Request Changes, with the pieces it
 *  actually uses: choose the agent, turn the review into messages, adopt-and-send. */
async function requestChanges(call, { projectId, worktreeId, provider, comments, general }) {
  const adopting = createAdoptingCall(call, projectId, worktreeId);
  const providerCatalog = catalogForProvider(CATALOG, provider);
  adopting.setAdoptParams(modelParams(providerCatalog.models, "", "", provider));
  const messages = diffThreadMessages(comments, general, null);
  await adopting.runCall("run.request_changes", { messages });
  return adopting;
}

describe("a review's comments reach the worktree's one agent", () => {
  it("adopts with the agent the sheet chose, then delivers the comments to it", async () => {
    const storage = memoryStorage();
    markNewWorktree("wt-abc", "codex", storage); // the New Worktree sheet's answer
    const provider = takeNewWorktreeMark("wt-abc", storage);

    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-4" } : { ok: true }));
    await requestChanges(call, {
      projectId: "proj-1",
      worktreeId: "wt-abc",
      provider,
      comments: COMMENTS,
      general: "and rename the module",
    });

    expect(call).toHaveBeenNthCalledWith(1, "run.adopt", {
      project_id: "proj-1",
      worktree_id: "wt-abc",
      provider: "codex",
    });
    const [method, params] = call.mock.calls[1];
    expect(method).toBe("run.request_changes");
    expect(params.run_id).toBe("run-4");
    // The reviewer's words travel as durable thread messages, not as an argv
    // string: the agent pulls them with read_unread_messages, whether it was
    // already running in that tab or is being started for them.
    expect(params.messages.map((m) => m.body)).toEqual(
      expect.arrayContaining(["name this", "and rename the module"]),
    );
    // Each comment keeps a source anchor, so the agent is told WHERE, not just what.
    expect(params.messages[0].anchor).toMatchObject({ artifact: "diff", path: "src/app.rs", line_start: 10, line_end: 12 });
  });

  it("keeps every later send on the same run — one worktree, one agent", async () => {
    const call = vi.fn(async (method) => (method === "run.adopt" ? { run_id: "run-4" } : { ok: true }));
    const adopting = await requestChanges(call, {
      projectId: "p",
      worktreeId: "wt-abc",
      provider: "claude",
      comments: COMMENTS,
      general: "",
    });
    await adopting.runCall("run.request_changes", { messages: diffThreadMessages([], "one more thing", null) });

    expect(call.mock.calls.filter(([method]) => method === "run.adopt")).toHaveLength(1);
    expect(call.mock.calls.filter(([method]) => method === "run.request_changes")).toHaveLength(2);
    for (const [method, params] of call.mock.calls) {
      if (method === "run.request_changes") expect(params.run_id).toBe("run-4");
    }
  });

  it("leaves the worktree un-adopted when the adopt fails, so nothing is half-bound", async () => {
    const call = vi.fn(async (method) => {
      if (method === "run.adopt") throw new Error("worktree is on the base branch");
      return { ok: true };
    });
    const adopting = createAdoptingCall(call, "p", "wt-abc");
    adopting.setAdoptParams({ provider: "claude" });
    await expect(adopting.runCall("run.request_changes", { messages: [] })).rejects.toThrow(/base branch/);
    expect(adopting.adoptedRunId()).toBe(null);
    expect(call.mock.calls.filter(([method]) => method === "run.request_changes")).toHaveLength(0);
  });
});
