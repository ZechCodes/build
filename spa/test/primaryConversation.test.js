// @vitest-environment jsdom
// The primary checkout's conversation — the main surface talking to the agent
// that runs in the repo root.
//
// The repo root is a worktree like any other, so it gets what every other
// worktree surface has: a thread over an OWNER. It just may not have one yet.
// Un-adopted, the tab is a composer alone and the first message is what mints
// the run (adopt-on-first-mutation, the external-worktree gesture). ONE
// deliberate difference from an external worktree: nothing hands off afterwards
// — the primary run's home IS this surface, so the thread opens right here.
//
// Opening the tab must never adopt: mounting is a look, and a look that minted
// an owner would make every reload a mutation.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { mountPrimaryConversation } from "../src/views/mainWorktree.js";
import { createPrimaryAdoptingCall } from "../src/core/adoption.js";
import { primaryRunIdFor } from "../src/core/taskFeed.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const host = () => document.body.appendChild(document.createElement("div"));

const threadOf = (...bodies) => ({
  items: bodies.map((body, index) => ({
    type: "message",
    data: { role: "agent", body, sequence: index + 1, created_at: "2026-08-06T12:00:00Z" },
  })),
});

/** A bridge double: run.adopt mints one run, run.get serves whatever view the
 *  test set, everything else acknowledges. */
const fakeBridge = (view = { state: "review", harness: "Claude Code", thread: threadOf("on it") }) => {
  const calls = [];
  const call = vi.fn(async (method, params) => {
    calls.push([method, params]);
    if (method === "run.adopt") return { run_id: "run-main", state: "review" };
    if (method === "run.get") return bridge.view;
    return { ok: true };
  });
  const bridge = {
    call,
    calls,
    view,
    of: (method) => calls.filter(([m]) => m === method).map(([, params]) => params),
  };
  return bridge;
};

const send = async (el, text) => {
  el.querySelector("#mainthreadinput").value = text;
  el.querySelector("#mainthreadsend").click();
  await tick();
  await tick();
};

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the primary checkout's conversation", () => {
  it("offers a composer and adopts nothing when the checkout has no owner yet", async () => {
    const bridge = fakeBridge();
    const adopting = createPrimaryAdoptingCall(bridge.call, "proj-1");
    const el = host();

    const pane = mountPrimaryConversation(el, { adopting, callRpc: bridge.call, pollMs: 0 });
    await tick();

    expect(el.querySelector("#mainthreadinput")).toBeTruthy();
    expect(bridge.of("run.adopt")).toHaveLength(0);
    expect(bridge.of("run.get")).toHaveLength(0);
    pane.dispose();
  });

  it("mints the run on the first message, posts to it, and opens its thread here", async () => {
    const bridge = fakeBridge();
    const adopting = createPrimaryAdoptingCall(bridge.call, "proj-1");
    const el = host();
    const pane = mountPrimaryConversation(el, { adopting, callRpc: bridge.call, pollMs: 0 });

    await send(el, "look at the failing test");

    expect(bridge.of("run.adopt")).toEqual([{ project_id: "proj-1", primary: true }]);
    expect(bridge.of("thread.post")).toEqual([{ entity_id: "run-main", body: "look at the failing test" }]);
    // Stays on the main surface: the thread the adopt opened renders in place.
    expect(bridge.of("run.get")[0].run_id).toBe("run-main");
    expect(el.textContent).toContain("on it");
    expect(el.querySelector("#mainthreadinput")).toBeTruthy();
    pane.dispose();
  });

  // Reconnect (reload, daemon restart): the owner is learned read-only and the
  // pane binds to it — the thread is live without anything being minted.
  it("reads an existing owner's thread without adopting", async () => {
    const bridge = fakeBridge({ state: "review", harness: "Codex", thread: threadOf("resumed") });
    const adopting = createPrimaryAdoptingCall(bridge.call, "proj-1");
    adopting.seedAdoptedRun("run-existing");
    const el = host();

    const pane = mountPrimaryConversation(el, { adopting, callRpc: bridge.call, pollMs: 0 });
    await tick();

    expect(bridge.of("run.adopt")).toHaveLength(0);
    expect(bridge.of("run.get")[0].run_id).toBe("run-existing");
    expect(el.textContent).toContain("resumed");
    pane.dispose();
  });

  // A terminal run has let go of the checkout, so the surface goes back to the
  // invitation and the next message adopts a fresh owner.
  it("lets go of a run that has ended and adopts again on the next message", async () => {
    const bridge = fakeBridge({ state: "abandoned", harness: "Claude Code", thread: threadOf("done here") });
    const adopting = createPrimaryAdoptingCall(bridge.call, "proj-1");
    adopting.seedAdoptedRun("run-old");
    const el = host();
    const pane = mountPrimaryConversation(el, { adopting, callRpc: bridge.call, pollMs: 0 });
    await tick();

    expect(adopting.adoptedRunId()).toBe(null);

    bridge.view = { state: "review", harness: "Claude Code", thread: threadOf("on it") };
    await send(el, "next round");

    expect(bridge.of("run.adopt")).toEqual([{ project_id: "proj-1", primary: true }]);
    expect(bridge.of("thread.post")).toEqual([{ entity_id: "run-main", body: "next round" }]);
    pane.dispose();
  });

  it("keeps the draft the surface holds so a tab switch does not lose it", async () => {
    const bridge = fakeBridge();
    const adopting = createPrimaryAdoptingCall(bridge.call, "proj-1");
    const el = host();
    let draft = "half a thought";

    const pane = mountPrimaryConversation(el, {
      adopting,
      callRpc: bridge.call,
      pollMs: 0,
      readDraft: () => draft,
      writeDraft: (value) => {
        draft = value;
      },
    });
    await tick();

    expect(el.querySelector("#mainthreadinput").value).toBe("half a thought");
    pane.dispose();
  });
});

// Which run owns a project's primary checkout, read off the feed the sidebar
// already polls — a read, never an adopt.
describe("primaryRunIdFor", () => {
  it("names the run the bridge reports as the checkout's owner", () => {
    const feed = {
      primaryChanges: [
        { project_id: "proj-other", run_id: "run-other" },
        { project_id: "proj-1", run_id: "run-main" },
      ],
    };
    expect(primaryRunIdFor(feed, "proj-1")).toBe("run-main");
  });

  it("is null while nobody owns the checkout", () => {
    expect(primaryRunIdFor({ primaryChanges: [{ project_id: "proj-1", run_id: null }] }, "proj-1")).toBe(null);
    expect(primaryRunIdFor({ primaryChanges: [] }, "proj-1")).toBe(null);
    expect(primaryRunIdFor(null, "proj-1")).toBe(null);
  });
});
