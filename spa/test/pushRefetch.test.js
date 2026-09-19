// @vitest-environment jsdom
// A surface refetching on a push, end to end: real surfaces mounted over a
// scripted RPC channel, with the bridge's change events arriving on them.
//
// The point of the file is that an event is the whole of it. A surface that
// reads its own checkout — a project's own directory, which is no entity the
// board names, or a workspace source, which the sync layer does not walk —
// reads it when the bridge says it moved, and at no other time. There is no
// clock left in any of them.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const status = {
  branch: "build/login",
  path: "/repo",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/build/login",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [],
  files_truncated: false,
  stat: { files_changed: 0, insertions: 0, deletions: 0 },
  patch: "",
  truncated: false,
};

const log = { branch: "build/login", commits: [], more: false };

let mountGitPane;
let armChangeEvents, dispatchChangeEvent, refetchEverything, resetChangeEvents, SAFETY_POLL_MS;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = "";
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
  ({
    armChangeEvents,
    dispatchChangeEvent,
    refetchEverything,
    resetChangeEvents,
    SAFETY_POLL_MS,
  } = await import("../src/core/changeEvents.js"));
});

afterEach(() => {
  resetChangeEvents();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

/** The Changes pane over a scripted channel, with its reads counted. One poll
 *  of this surface is exactly one git.status. */
async function mountPane(scope = { run_id: "run-7" }) {
  const callRpc = vi.fn(async (method) => {
    if (method === "git.status") return status;
    if (method === "git.log") return log;
    return {};
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope, callRpc });
  await settle();
  const reads = () => callRpc.mock.calls.filter(([method]) => method === "git.status").length;
  return { pane, reads };
}

/** One flush off a machine: the bodies of what moved, addressed by entity. */
const flush = (items, deviceId) => dispatchChangeEvent({ type: "changes", items }, deviceId);

/** The board's own line, and one entity's, as the bridge addresses them. */
const boardMoved = (deviceId) => flush([{ entity_id: "board", state: {} }], deviceId);
const entityMoved = (entityId, deviceId) => flush([{ entity_id: entityId, git: {} }], deviceId);

describe("a surface against a bridge that pushes", () => {
  it("reads again when the entity it is showing changes", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane();
    const before = reads();

    entityMoved("run-7", "dev-a");
    await settle();
    expect(reads()).toBe(before + 1);
    pane.dispose();
  });

  it("ignores an entity it is not showing", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane();
    const before = reads();

    entityMoved("run-99", "dev-a");
    await settle();
    expect(reads()).toBe(before);
    pane.dispose();
  });

  it("ignores the board's own item — the feed moved, not this entity's detail", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane();
    const before = reads();

    boardMoved("dev-a");
    await settle();
    expect(reads()).toBe(before);
    pane.dispose();
  });

  it("watches the board when its scope is a project checkout, which names no entity", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane({ project_id: "p1" });
    const before = reads();

    boardMoved("dev-a");
    await settle();
    expect(reads()).toBe(before + 1);
    pane.dispose();
  });

  it("watches the board when its scope is a workspace source, which the bridge names no entity for", async () => {
    // A durable workspace id is not an id the bridge ever puts on an item:
    // its git subjects are runs, projects and external worktrees, and a
    // workspace source's file writes note no entity at all. The board is the
    // only word about this checkout that ever arrives — and the workspace's
    // own agent moves it on every turn.
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane({ workspace_id: "ws-1", source_id: "src-1" });
    const before = reads();

    boardMoved("dev-a");
    await settle();
    expect(reads()).toBe(before + 1);
    pane.dispose();
  });

  it("has no clock at all, not even the safety poll's", async () => {
    // Before the mount: an interval, if there were one, would be the fake one
    // from the start.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane();
    const before = reads();

    await vi.advanceTimersByTimeAsync(SAFETY_POLL_MS * 4);
    expect(reads()).toBe(before);
    pane.dispose();
  });

  it("reads once for a reconnect, whatever it is showing", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const entity = await mountPane();
    const board = await mountPane({ project_id: "p1" });
    const before = [entity.reads(), board.reads()];

    refetchEverything();
    await settle();
    expect([entity.reads(), board.reads()]).toEqual([before[0] + 1, before[1] + 1]);
    entity.pane.dispose();
    board.pane.dispose();
  });

  it("stops hearing events once the surface is disposed", async () => {
    armChangeEvents({ push_events: true }, "dev-a");
    const { pane, reads } = await mountPane();
    pane.dispose();
    const before = reads();

    entityMoved("run-7", "dev-a");
    await settle();
    expect(reads()).toBe(before);
  });
});

describe("a surface against a bridge that does not", () => {
  // Nothing replaces the poll for a bridge with no pushes: the surface paints
  // what it read on mount, and a reconnect is what reads again.
  it("waits rather than polling", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { pane, reads } = await mountPane();
    const before = reads();

    await vi.advanceTimersByTimeAsync(SAFETY_POLL_MS * 4);
    expect(reads()).toBe(before);
    pane.dispose();
  });

  it("does not read on an event it was never told to expect", async () => {
    const { pane, reads } = await mountPane();
    const before = reads();

    entityMoved("run-7", "dev-a");
    boardMoved("dev-a");
    await settle();
    expect(reads()).toBe(before);
    pane.dispose();
  });
});
