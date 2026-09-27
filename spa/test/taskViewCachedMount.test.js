// @vitest-environment jsdom
// Mounting the task surface over records that were already on disk.
//
// Tasks left the board, so nothing fills their records for them: no ordered
// pass, no boot sync, and a push only where the task moves while this tab is
// watching it. That makes the mount the one moment the surface can catch up on
// what happened while it was closed — an approval given from another device,
// a stage implemented on the machine itself — because no later word will ever
// name it.
//
// So: the records paint the first frame, and the machine is asked straight
// after. Both, in that order. Waiting on the read would give back the round
// trip this stage removed.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let mountTaskView, taskAddress, writeCached;

const taskPayload = (overrides = {}) => ({
  task_id: "task-1",
  plan_id: "task-1",
  project_id: "proj-1",
  project: "Build",
  goal: "Rebuild the task view",
  state: "plan_review",
  base_branch: "main",
  docs_available: false,
  stages: [{ id: "s1", state: "planned" }],
  implementation_lineage: [],
  thread: { items: [], thread_last_sequence: 4 },
  ...overrides,
});

const stage = (overrides = {}) => ({
  id: "s1",
  title: "Wire",
  state: "planned",
  approval: "planned",
  execution: "pending",
  open_comments: 0,
  comments: [],
  ...overrides,
});

let host = null;
let view = null;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ mountTaskView } = await import("../src/core/taskView.js"));
  ({ taskAddress } = await import("../src/core/taskCache.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  host = document.createElement("div");
  document.body.appendChild(host);
});

afterEach(() => {
  view?.dispose();
  view = null;
  host?.remove();
});

const settle = async () => {
  for (let turn = 0; turn < 40; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

/** What the last session left on disk for this task. */
const seed = async (task, stages) => {
  await writeCached(taskAddress("dev-1", "task-1", "get"), task);
  await writeCached(taskAddress("dev-1", "task-1", "stages"), { stages });
};

describe("mounting over records the last session left", () => {
  it("paints them before the machine answers, and asks it anyway", async () => {
    await seed(taskPayload(), [stage()]);
    const calls = [];
    let release = null;
    const answered = new Promise((done) => {
      release = done;
    });
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: async (method) => {
        calls.push(method);
        await answered;
        if (method === "task.get") return taskPayload({ goal: "Rebuild the task view", state: "implementing" });
        if (method === "task.stages") return { stages: [stage({ approval: "approved", state: "approved" })] };
        return {};
      },
    });
    await settle();

    // The frame is up and what it says is the record's, with the machine still
    // holding its answer.
    expect(host.querySelector(".ivsplit")).toBeTruthy();
    expect(host.textContent).toContain("PLANNED");
    expect(calls).toContain("task.get");

    release();
    await settle();

    // And what the machine says lands over it: the stage was approved from
    // another device while this tab was closed, and no push will ever say so.
    expect(host.textContent).not.toContain("PLANNED");
  });

  it("reads each of them once, not once per paint", async () => {
    await seed(taskPayload(), [stage()]);
    const calls = [];
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: async (method) => {
        calls.push(method);
        if (method === "task.get") return taskPayload();
        if (method === "task.stages") return { stages: [stage()] };
        return {};
      },
    });
    await settle();

    expect(calls.filter((method) => method === "task.get")).toHaveLength(1);
    expect(calls.filter((method) => method === "task.stages")).toHaveLength(1);
  });

  it("repaints from external cache writes while every payload stays absent", async () => {
    await seed(taskPayload({ docs_available: true }), [stage()]);
    await writeCached(taskAddress("dev-1", "task-1", "stage:s1"), {
      stage_id: "s1",
      contents: "# Cached first cut",
    });
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: () => new Promise(() => {}),
    });
    await settle();
    expect(host.textContent).toContain("Cached first cut");

    await writeCached(taskAddress("dev-1", "task-1", "stages"), {
      stages: [stage({ title: "Rewired through cache" })],
    });
    await writeCached(taskAddress("dev-1", "task-1", "stage:s1"), {
      stage_id: "s1",
      contents: "# Cache announced revision",
    });
    await settle();

    expect(host.textContent).toContain("Rewired through cache");
    expect(host.textContent).toContain("Cache announced revision");
  });

  it("keeps an opened stable diff cache-driven across unrelated cache writes", async () => {
    const complete = stage({
      state: "complete",
      approval: "approved",
      execution: "complete",
      start_sha: "aaa",
      completion_sha: "bbb",
    });
    const task = taskPayload({ state: "implementing", stages: [complete] });
    await seed(task, [complete]);
    await writeCached(taskAddress("dev-1", "task-1", "stage:s1"), { stage_id: "s1", contents: "# Done" });
    await writeCached(taskAddress("dev-1", "task-1", "stagediff:s1"), {
      status: "available",
      patch: "first cached patch",
    });
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: () => new Promise(() => {}),
    });
    await settle();
    host.querySelector("#stagediff").click();
    await settle();
    expect(host.querySelector("#stagediffpane").textContent).toContain("first cached patch");

    await writeCached(taskAddress("dev-1", "task-1", "stagediff:s1"), {
      status: "available",
      patch: "replacement cached patch",
    });
    await writeCached(taskAddress("dev-1", "task-1", "get"), { ...task, implementation_activity: "settled" });
    await settle();

    expect(host.querySelector("#stagediffpane").textContent).toContain("replacement cached patch");
  });
});

// A stage doc is not written once and kept forever. A plan is revised while the
// reader has it open (bridge `plan.rs`: `StageDocEvent::Revised` is legal from
// both Planned and Approved), and the word that says so is the same one the
// task itself is re-read on — a `state` push naming the task, or the mount.
// Nothing else ever fills these records, so a doc that is not read again on
// that word is never read again at all.
describe("the records the surface fills on demand", () => {
  const revisable = () => {
    const stages = [stage({ id: "s2" })];
    return { stages, task: taskPayload({ docs_available: true, stages }) };
  };

  it("reads the open stage's doc again when a word says the task moved", async () => {
    const { stages, task } = revisable();
    await seed(task, stages);
    await writeCached(taskAddress("dev-1", "task-1", "stage:s2"), { stage_id: "s2", contents: "the first cut" });
    const calls = [];
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: async (method) => {
        calls.push(method);
        if (method === "task.get") return task;
        if (method === "task.stages") return { stages };
        if (method === "task.stage_doc") return { stage_id: "s2", contents: "what the planner rewrote" };
        return {};
      },
    });
    await settle();

    expect(calls.filter((method) => method === "task.stage_doc")).toHaveLength(1);
    expect(host.textContent).toContain("what the planner rewrote");
    expect(host.textContent).not.toContain("the first cut");
  });

  it("leaves the doc alone on a paint no word came with", async () => {
    const { stages, task } = revisable();
    await seed(task, stages);
    await writeCached(taskAddress("dev-1", "task-1", "stage:s2"), { stage_id: "s2", contents: "the first cut" });
    const calls = [];
    view = mountTaskView(host, {
      taskId: "task-1",
      projectId: "proj-1",
      deviceId: "dev-1",
      callRpc: async (method) => {
        calls.push(method);
        if (method === "task.get") return task;
        if (method === "task.stages") return { stages };
        if (method === "task.stage_doc") return { stage_id: "s2", contents: "the first cut" };
        return {};
      },
    });
    await settle();
    await settle();

    // One read: the mount's catch-up. Every frame after it is the record's.
    expect(calls.filter((method) => method === "task.stage_doc")).toHaveLength(1);
  });
});
