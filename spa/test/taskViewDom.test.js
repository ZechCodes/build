// @vitest-environment jsdom
// The legacy plan page as it behaves: two persistent columns, a stage
// selection that never costs the list, and nothing that would ask the bridge
// to change a plan — every such verb is refused, so the page is read-only.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTaskView, openingStageId } from "../src/core/taskView.js";
import { createAgentSelection } from "../src/core/agentSelection.js";
import { dismissAllNotices } from "../src/core/notify.js";
import { armChangeEvents, dispatchChangeEvent, resetChangeEvents } from "../src/core/changeEvents.js";
import { wipeCache } from "../src/core/localCache.js";

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

const taskPayload = (overrides = {}) => ({
  task_id: "task-1",
  plan_id: "task-1",
  project_id: "proj-1",
  project: "Build",
  goal: "Rebuild the task view",
  state: "plan_review",
  base_branch: "main",
  docs_available: true,
  stages: [{ id: "s1", state: "planned" }],
  implementation_lineage: [],
  thread: { items: [], thread_last_sequence: 4 },
  ...overrides,
});

/** Mount the view over a scripted bridge and let its first pass settle. */
async function mount(overrides = {}) {
  const calls = [];
  const {
    task = taskPayload(),
    stages = [stage()],
    doc = { stage_id: "s1", contents: "# Wire\n\nRewrite the client so it reads items[]." },
    planDoc = { contents: "# The whole plan" },
    fail = null,
    hold = null,
    timeout = null,
    beforeReply = null,
    ...options
  } = overrides;
  const host = document.createElement("div");
  document.body.appendChild(host);
  const view = mountTaskView(host, {
    taskId: "task-1",
    projectId: "proj-1",
    callRpc: async (method, params) => {
      calls.push([method, params]);
      if (fail && fail[method]) throw new Error(fail[method]);
      if (timeout && timeout[method]) {
        const timedOut = new Error(`${method} timed out`);
        timedOut.timedOut = true;
        timedOut.uncertain = true;
        throw timedOut;
      }
      if (hold && hold[method]) return new Promise(() => {});
      if (beforeReply) await beforeReply(method, params);
      if (method === "task.get") return task;
      if (method === "task.stages") return { stages };
      if (method === "task.stage_doc") return doc;
      if (method === "task.doc") return planDoc;
      return {};
    },
    ...options,
  });
  await settle(host);
  return { host, view, calls };
}

const settle = async (host) => {
  for (let i = 0; i < 30; i++) {
    await new Promise((done) => setTimeout(done, 0));
    if (host.querySelector(".ivsplit")) break;
  }
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

const flush = async () => {
  for (let i = 0; i < 30; i++) await new Promise((done) => setTimeout(done, 0));
};

/** The push that says this task moved: what wakes the surface now that
 *  nothing polls it. */
const pushMoved = async () => {
  armChangeEvents({ push_events: true });
  dispatchChangeEvent({ type: "changes", items: [{ entity_id: "task-1", state: { kind: "task" } }] });
  for (let i = 0; i < 40; i++) await new Promise((done) => setTimeout(done, 0));
};

/** Every `task.get` made for `agentId` so far, once at least one has been. The
 *  poll's own tick is the machine's business — how many times it has come round
 *  by any given millisecond is not something a test can name — so a test about
 *  which read carries what waits for the read rather than for a wall clock. */
const readsForAgent = async (calls, agentId) => {
  const deadline = Date.now() + 2000;
  for (;;) {
    const reads = calls.filter(([method, params]) => method === "task.get" && params.agent_id === agentId);
    if (reads.length) return reads;
    if (Date.now() > deadline) throw new Error(`the poll never read the task for ${agentId}`);
    await new Promise((resolve) => setTimeout(resolve, 1));
    await flush();
  }
};

describe("openingStageId", () => {
  const stages = [stage({ id: "a", state: "approved", approval: "approved" }), stage({ id: "b" })];

  it("keeps a deep-linked stage that still exists", () => {
    expect(openingStageId(stages, "b")).toBe("b");
  });

  it("opens on the first stage still awaiting approval when nothing was linked", () => {
    expect(openingStageId(stages, null)).toBe("b");
    expect(openingStageId(stages, "gone")).toBe("b");
  });

  it("falls back to the first stage when every plan is approved", () => {
    expect(openingStageId([stage({ id: "a", state: "approved", approval: "approved" })], null)).toBe("a");
    expect(openingStageId([], null)).toBeNull();
  });
});

describe("the task view", () => {
  beforeEach(async () => {
    if (typeof indexedDB === "undefined") globalThis.indexedDB = new IDBFactory();
    globalThis.IDBKeyRange = IDBKeyRange;
    await wipeCache();
    document.body.innerHTML = "";
  });
  afterEach(() => {
    resetChangeEvents();
    vi.restoreAllMocks();
    dismissAllNotices();
  });

  it("paints both columns at once — the stage list beside the open stage's doc", async () => {
    const { host, view } = await mount();
    expect(host.querySelector(".ivstages #stagelist")).toBeTruthy();
    expect(host.querySelector(".ivviewer #stagedoc")).toBeTruthy();
    expect(host.querySelector(".ivviewer").textContent).toContain("Wire");
    // No tabs, and no way back to a list that never left.
    expect(host.querySelector(".tabrow")).toBeNull();
    expect(host.querySelector(".stageback")).toBeNull();
    view.dispose();
  });

  // Plans are history: the bridge refuses every verb that would move one, so
  // the page is a record of what was planned and built, and offers nothing
  // that would ask it to change.
  describe("a read-only record", () => {
    const MUTATION_CONTROLS = [
      "#taskdelete",
      "#approvetask",
      "#approveall",
      "#implementall",
      "#approvestage",
      "#implementstage",
      "#sendnotes",
      "#assigntoggle",
      ".ivassign",
      ".cc-x",
      ".cssend",
      ".csgeneral",
    ];
    const controlsOn = (host) => MUTATION_CONTROLS.filter((selector) => host.querySelector(selector));

    it("offers no mutation on a plan under review with comments open", async () => {
      const commented = stage({
        open_comments: 1,
        comments: [{ id: "message-7", body: "why", state: "open", anchor: { heading_path: ["Wire"] } }],
      });
      const { host, view } = await mount({ stages: [commented, stage({ id: "s2" })] });
      expect(controlsOn(host)).toEqual([]);
      view.dispose();
    });

    it("offers no mutation on a ready plan whose next stage could have been implemented", async () => {
      const stages = [
        stage({ id: "s1", approval: "approved", state: "approved", execution: "complete" }),
        stage({ id: "s2", title: "Render", approval: "approved", state: "approved", execution: "pending" }),
      ];
      const { host, view } = await mount({ task: taskPayload({ state: "approved" }), stages });
      host.querySelectorAll(".stagerow")[1].click();
      await flush();
      expect(controlsOn(host)).toEqual([]);
      view.dispose();
    });

    it("offers no delete on an abandoned plan", async () => {
      const { host, view } = await mount({ task: taskPayload({ state: "abandoned" }) });
      expect(controlsOn(host)).toEqual([]);
      view.dispose();
    });

    it("sends nothing but reads", async () => {
      const { host, view, calls } = await mount({ stages: [stage(), stage({ id: "s2" })] });
      host.querySelectorAll(".stagerow")[1].click();
      await pushMoved();
      const reads = new Set(["entity.seen", "task.get", "task.stages", "task.doc", "task.stage_doc", "task.stage_diff"]);
      expect(calls.map(([method]) => method).filter((method) => !reads.has(method))).toEqual([]);
      view.dispose();
    });
  });

  it("opens the first stage awaiting approval and tells its host which one", async () => {
    const opened = [];
    const { view } = await mount({ onSelectStage: (id) => opened.push(id) });
    expect(opened).toEqual(["s1"]);
    view.dispose();
  });

  it("swaps the viewer on selection while the list stays put", async () => {
    const stages = [stage(), stage({ id: "s2", title: "Render" })];
    const { host, view, calls } = await mount({
      stages,
      doc: { stage_id: "s1", contents: "# Wire" },
    });
    host.querySelectorAll(".stagerow")[1].click();
    await flush();
    // Both stages are still listed, the open one is marked, and nothing offers
    // a way back to a list that never left.
    expect(host.querySelectorAll(".stagerow")).toHaveLength(2);
    expect(host.querySelectorAll(".stagerow")[1].classList.contains("sel")).toBe(true);
    expect(host.querySelector(".stageback")).toBeNull();
    expect(host.querySelector(".ivviewer").textContent).toContain("Render");
    expect(calls.some(([method, params]) => method === "task.stage_doc" && params.stage_id === "s2")).toBe(true);
    view.dispose();
  });

  // The stages rail is a drawer on a phone, so stepping to the next stage
  // through it costs a menu trip for the commonest move in a review. The steps
  // live on the doc pane's own bar instead, and they are the same selection the
  // rail makes.
  it("walks the stages in order from the doc pane, and the rail follows", async () => {
    const opened = [];
    const stages = [stage(), stage({ id: "s2", title: "Render" }), stage({ id: "s3", title: "Ship" })];
    const { host, view, calls } = await mount({ stages, onSelectStage: (id) => opened.push(id) });
    expect(host.querySelector(".stagenav-pos").textContent).toBe("1 / 3");

    host.querySelector('[data-stage-step="next"]').click();
    await flush();
    expect(host.querySelector(".ivviewer .ivstagetitle").textContent).toBe("Render");
    expect(host.querySelector(".stagenav-pos").textContent).toBe("2 / 3");
    // One selection, both columns: the rail marks the stage the steps opened.
    const selected = host.querySelectorAll(".stagerow.sel");
    expect(selected).toHaveLength(1);
    expect(selected[0].dataset.stage).toBe("s2");
    expect(calls.some(([method, params]) => method === "task.stage_doc" && params.stage_id === "s2")).toBe(true);

    host.querySelector('[data-stage-step="next"]').click();
    await flush();
    expect(host.querySelector(".stagenav-pos").textContent).toBe("3 / 3");

    host.querySelector('[data-stage-step="prev"]').click();
    await flush();
    expect(host.querySelector(".stagenav-pos").textContent).toBe("2 / 3");
    // The host's URL rode along with every step, and never doubled back.
    expect(opened).toEqual(["s1", "s2", "s3", "s2"]);
    view.dispose();
  });

  it("runs out of steps at both ends of the task", async () => {
    const stages = [stage(), stage({ id: "s2", title: "Render" })];
    const { host, view } = await mount({ stages });
    expect(host.querySelector('[data-stage-step="prev"]').disabled).toBe(true);
    expect(host.querySelector('[data-stage-step="next"]').disabled).toBe(false);
    host.querySelector('[data-stage-step="next"]').click();
    await flush();
    expect(host.querySelector('[data-stage-step="prev"]').disabled).toBe(false);
    expect(host.querySelector('[data-stage-step="next"]').disabled).toBe(true);
    view.dispose();
  });

  it("offers no steps on a task with a single stage", async () => {
    const { host, view } = await mount();
    expect(host.querySelector(".stagenav")).toBeNull();
    view.dispose();
  });

  it("leaves the steps alone across a push that changed nothing", async () => {
    const stages = [stage(), stage({ id: "s2", title: "Render" })];
    const { host, view } = await mount({ stages });
    const next = host.querySelector('[data-stage-step="next"]');
    await pushMoved();
    expect(host.contains(next), "an idempotent pass replaced the step the reader is aiming at").toBe(true);
    view.dispose();
  });

  // The rail is reconciled by stage id, not rewritten: a pass that had nothing
  // to say about the stages leaves every row where it stood, and a pass that
  // changed one stage touches only that stage's row.
  describe("the rail's paint", () => {
    /** Everything the DOM under `target` did while `act` ran. */
    const churn = async (target, act) => {
      const seen = [];
      const observer = new MutationObserver((records) => seen.push(...records));
      observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
      await act();
      seen.push(...observer.takeRecords());
      observer.disconnect();
      return seen;
    };

    const poll = () => pushMoved();

    it("leaves every stage row alone when the pass changed only the task", async () => {
      const stages = [stage(), stage({ id: "s2", title: "Render" })];
      const task = taskPayload({ stages: [{ id: "s1", state: "planned" }, { id: "s2", state: "planned" }] });
      const { host, view } = await mount({ task, stages });
      const rows = [...host.querySelectorAll(".stagerow")];
      const records = await churn(host.querySelector("#stagelist"), async () => {
        task.state = "approved";
        await poll();
      });
      expect(host.querySelector(".ivhead").textContent).toContain("APPROVED"); // the head did move
      expect(records).toEqual([]);
      expect([...host.querySelectorAll(".stagerow")]).toEqual(rows);
      view.dispose();
    });

    it("redraws only the stage that changed", async () => {
      const stages = [stage(), stage({ id: "s2", title: "Render" })];
      const { host, view } = await mount({ stages });
      const first = host.querySelector('.stagerow[data-stage="s1"]');
      const second = host.querySelector('.stagerow[data-stage="s2"]');
      const records = await churn(host.querySelector("#stagelist"), async () => {
        stages[1].title = "Render, twice";
        await poll();
      });
      expect(host.querySelector('.stagerow[data-stage="s1"]')).toBe(first);
      expect(host.querySelector('.stagerow[data-stage="s2"]')).toBe(second);
      expect(second.textContent).toContain("Render, twice");
      expect(records.length).toBeGreaterThan(0);
      expect(records.every((record) => second.contains(record.target))).toBe(true);
      view.dispose();
    });

    it("moves the mark on a selection without rebuilding a row", async () => {
      const stages = [stage(), stage({ id: "s2", title: "Render" })];
      const { host, view } = await mount({ stages });
      const rows = [...host.querySelectorAll(".stagerow")];
      rows[1].click();
      await flush();
      expect([...host.querySelectorAll(".stagerow")]).toEqual(rows);
      expect(rows[0].classList.contains("sel")).toBe(false);
      expect(rows[1].classList.contains("sel")).toBe(true);
      view.dispose();
    });
  });

  // The daemon revises a stage doc from planned and from approved alike
  // (`StageDocEvent::Revised`), and the push naming the task is the only word
  // that ever says so: nothing fills a task's records but this surface.
  it("reads the open stage's doc again when a push says the task moved", async () => {
    const doc = { stage_id: "s1", contents: "# Wire\n\nthe first cut of the plan." };
    const { host, view, calls } = await mount({ doc });
    expect(host.textContent).toContain("the first cut of the plan.");

    doc.contents = "# Wire\n\nwhat the planner rewrote.";
    await pushMoved();

    expect(calls.filter(([method]) => method === "task.stage_doc")).toHaveLength(2);
    expect(host.textContent).toContain("what the planner rewrote.");
    expect(host.textContent).not.toContain("the first cut of the plan.");
    view.dispose();
  });

  // There is no validation gate: a stage is building, then complete. A
  // complete stage offers its stable diff, and nothing offers to send a stage
  // "back to fix" — that verb went with the gate.
  it("offers a complete stage's stable diff and never a send-back-to-fix", async () => {
    const complete = stage({ approval: "approved", state: "approved", execution: "complete", start_sha: "a".repeat(40), completion_sha: "b".repeat(40) });
    const { host, view } = await mount({ task: taskPayload({ state: "approved" }), stages: [complete] });
    expect(host.querySelector("#stagediff")).toBeTruthy();
    expect(host.querySelector("#fixstage")).toBeNull();
    expect(host.querySelector(".ivviewer").textContent).toContain("COMPLETE");
    view.dispose();
  });

  it("offers no send-back-to-fix on an incomplete stage either", async () => {
    const incomplete = stage({ approval: "approved", state: "approved", execution: "incomplete", invalidation_reason: "worktree moved" });
    const { host, view } = await mount({ task: taskPayload({ state: "approved" }), stages: [incomplete] });
    expect(host.querySelector("#fixstage")).toBeNull();
    view.dispose();
  });

  it("hangs the comments already on the doc in its margin, and offers no way to withdraw one", async () => {
    const commented = stage({
      open_comments: 1,
      comments: [
        { id: "message-7", body: "why", state: "open", anchor: { heading_path: ["Wire"], snippet: "items[]", line_start: 3, line_end: 3 } },
      ],
    });
    const { host, view, calls } = await mount({ stages: [commented] });
    const marker = host.querySelector(".docmarker");
    expect(marker).toBeTruthy();
    expect(marker.dataset.marker).toBe("wire");
    expect(host.querySelector('.commentcard[data-id="message-7"]')).toBeTruthy();
    expect(host.querySelector(".commentcard .cc-x")).toBeNull();
    expect(calls.map(([method]) => method)).not.toContain("task.comment_delete");
    view.dispose();
  });

  it("renders the implementation lineage under the stages and opens a branch's changes", async () => {
    const routed = [];
    const { host, view } = await mount({
      task: taskPayload({
        implementation_lineage: [
          { run_id: "run-1", state: "merged", branch: "build/one" },
          { run_id: "run-2", state: "building", branch: "build/two" },
        ],
      }),
      navigate: (route) => routed.push(route),
    });
    const rows = host.querySelectorAll(".ivlin-row");
    expect(rows).toHaveLength(2);
    expect(rows[1].textContent).toContain("build/two");
    rows[1].click();
    expect(routed).toEqual([{ name: "branch", projectId: "proj-1", branch: "build/two", tab: "changes" }]);
    view.dispose();
  });

  it("reads the plan of a task that has no stages at all", async () => {
    const { host, view } = await mount({ task: taskPayload({ stages: [] }), stages: [] });
    expect(host.querySelector(".ivviewer").textContent).toContain("The whole plan");
    expect(host.querySelector("#stagelist").textContent).toContain("No stages yet");
    view.dispose();
  });

  it("says the agent is still drafting rather than offering a plan that does not exist", async () => {
    const { host, view, calls } = await mount({ task: taskPayload({ state: "drafting", stages: [] }), stages: [] });
    expect(host.querySelector(".ivviewer").textContent).toContain("drafting the plan");
    expect(calls.some(([method]) => method === "task.stage_doc")).toBe(false);
    view.dispose();
  });

  it("does not claim an agent is drafting a task nothing has started on", async () => {
    // A created task is inert: the router files it and nothing runs until the
    // first message. Saying an agent is drafting would put a worker on the
    // surface that does not exist.
    const { host, view, calls } = await mount({ task: taskPayload({ state: "created", stages: [] }), stages: [] });
    const viewer = host.querySelector(".ivviewer").textContent;
    expect(viewer).not.toContain("drafting the plan");
    expect(viewer).toContain("first message");
    expect(calls.some(([method]) => method === "task.stage_doc")).toBe(false);
    view.dispose();
  });

  it("latches a deleted task instead of repainting over it", async () => {
    const gone = [];
    const { host, view, calls } = await mount({
      fail: { "task.get": "unknown task_id: task-1" },
      onGone: () => gone.push(true),
    });
    expect(host.textContent).toContain("This Task no longer exists");
    const fetched = calls.filter(([method]) => method === "task.get").length;
    await flush();
    expect(calls.filter(([method]) => method === "task.get").length).toBe(fetched);
    host.querySelector("#goneback").click();
    expect(gone).toEqual([true]);
    view.dispose();
  });

  it("names a bound on every task.get, so no read of it ships a conversation whole", async () => {
    // The only thing this surface reads off the payload's thread is the newest
    // sequence — the rail beside it owns what gets rendered. A read that names
    // no bound gets every item the conversation ever held, over E2EE, to
    // compute one integer.
    const selection = createAgentSelection("agent:one");
    const { view, calls } = await mount({ agentSelection: selection });
    const reads = calls.filter(([method]) => method === "task.get");
    expect(reads).toHaveLength(1);
    expect(reads[0][1].thread_limit).toBe(1);
    expect(reads[0][1].thread_after_sequence).toBeUndefined();
    view.dispose();
  });

  it("names the same bound on the read after a bubble switch", async () => {
    const selection = createAgentSelection("agent:one");
    const { view, calls } = await mount({ agentSelection: selection });
    const readsBeforeSwitch = calls.filter(([method]) => method === "task.get").length;

    selection.set("agent:two");
    await pushMoved();

    const afterSwitch = (await readsForAgent(calls, "agent:two"))[0][1];
    expect(calls.filter(([method]) => method === "task.get").length).toBeGreaterThan(readsBeforeSwitch);
    expect(afterSwitch.thread_limit).toBe(1);
    expect(afterSwitch.thread_after_sequence).toBeUndefined();
    view.dispose();
  });

  it("renders the doc's error state with a Retry that clears the latch", async () => {
    const { host, view, calls } = await mount({ fail: { "task.stage_doc": "read failed" } });
    expect(host.querySelector("#stagedocretry")).toBeTruthy();
    const reads = calls.filter(([method]) => method === "task.stage_doc").length;
    expect(reads).toBe(1); // latched off: nothing retries it
    view.dispose();
  });
});
