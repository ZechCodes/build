// @vitest-environment jsdom
// The issue view as it behaves: two persistent columns, a stage selection that
// never costs the list, doc comments that go out as anchored conversation
// messages, and the assignment control feeding the implement verbs.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountIssueView, openingStageId, docAnnotatable } from "../src/core/issueView.js";
import { FIRST_PAGE_ITEMS } from "../src/core/thread.js";
import { createAgentSelection } from "../src/core/agentSelection.js";

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

const issuePayload = (overrides = {}) => ({
  issue_id: "issue-1",
  plan_id: "issue-1",
  project_id: "proj-1",
  project: "Build",
  goal: "Rebuild the issue view",
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
    issue = issuePayload(),
    stages = [stage()],
    doc = { stage_id: "s1", contents: "# Wire\n\nRewrite the client so it reads items[]." },
    planDoc = { contents: "# The whole plan" },
    fail = null,
    ...options
  } = overrides;
  const host = document.createElement("div");
  document.body.appendChild(host);
  const view = mountIssueView(host, {
    issueId: "issue-1",
    projectId: "proj-1",
    callRpc: async (method, params) => {
      calls.push([method, params]);
      if (fail && fail[method]) throw new Error(fail[method]);
      if (method === "issue.get") return issue;
      if (method === "issue.stages") return { stages };
      if (method === "issue.stage_doc") return doc;
      if (method === "issue.doc") return planDoc;
      return {};
    },
    ...options,
  });
  await settle(host);
  return { host, view, calls };
}

const settle = async (host) => {
  for (let i = 0; i < 30; i++) {
    await Promise.resolve();
    if (host.querySelector(".ivsplit")) break;
  }
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

const flush = async () => {
  for (let i = 0; i < 30; i++) await Promise.resolve();
};

/** Every `issue.get` made for `agentId` so far, once at least one has been. The
 *  poll's own tick is the machine's business — how many times it has come round
 *  by any given millisecond is not something a test can name — so a test about
 *  which read carries what waits for the read rather than for a wall clock. */
const readsForAgent = async (calls, agentId) => {
  const deadline = Date.now() + 2000;
  for (;;) {
    const reads = calls.filter(([method, params]) => method === "issue.get" && params.agent_id === agentId);
    if (reads.length) return reads;
    if (Date.now() > deadline) throw new Error(`the poll never read the issue for ${agentId}`);
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

describe("docAnnotatable", () => {
  it("takes comments on a readable planned or approved doc only", () => {
    expect(docAnnotatable(stage(), "ready")).toBe(true);
    expect(docAnnotatable(stage({ approval: "approved" }), "ready")).toBe(true);
    expect(docAnnotatable(stage(), "loading")).toBe(false);
    expect(docAnnotatable(stage({ approval: "superseded" }), "ready")).toBe(false);
    expect(docAnnotatable(null, "ready")).toBe(false);
  });
});

describe("the issue view", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });
  afterEach(() => {
    vi.restoreAllMocks();
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
    expect(calls.some(([method, params]) => method === "issue.stage_doc" && params.stage_id === "s2")).toBe(true);
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
    expect(calls.some(([method, params]) => method === "issue.stage_doc" && params.stage_id === "s2")).toBe(true);

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

  it("runs out of steps at both ends of the issue", async () => {
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

  it("offers no steps on an issue with a single stage", async () => {
    const { host, view } = await mount();
    expect(host.querySelector(".stagenav")).toBeNull();
    view.dispose();
  });

  it("leaves the steps alone across a poll pass that changed nothing", async () => {
    const stages = [stage(), stage({ id: "s2", title: "Render" })];
    const { host, view } = await mount({ stages, pollMs: 5 });
    const next = host.querySelector('[data-stage-step="next"]');
    await new Promise((resolve) => setTimeout(resolve, 20));
    await flush();
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

    const poll = async () => {
      await new Promise((resolve) => setTimeout(resolve, 30));
      await flush();
    };

    it("leaves every stage row alone when the pass changed only the issue", async () => {
      const stages = [stage(), stage({ id: "s2", title: "Render" })];
      const issue = issuePayload({ stages: [{ id: "s1", state: "planned" }, { id: "s2", state: "planned" }] });
      const { host, view } = await mount({ issue, stages, pollMs: 5 });
      const rows = [...host.querySelectorAll(".stagerow")];
      const records = await churn(host.querySelector("#stagelist"), async () => {
        issue.state = "approved";
        await poll();
      });
      expect(host.querySelector(".ivhead").textContent).toContain("APPROVED"); // the head did move
      expect(records).toEqual([]);
      expect([...host.querySelectorAll(".stagerow")]).toEqual(rows);
      view.dispose();
    });

    it("redraws only the stage that changed", async () => {
      const stages = [stage(), stage({ id: "s2", title: "Render" })];
      const { host, view } = await mount({ stages, pollMs: 5 });
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

  it("approves every planned stage from the list, one call per stage", async () => {
    const { host, view, calls } = await mount({ stages: [stage(), stage({ id: "s2" })] });
    host.querySelector("#approveall").click();
    await flush();
    const approvals = calls.filter(([method]) => method === "issue.stage_approve").map(([, params]) => params.stage_id);
    expect(approvals).toEqual(["s1", "s2"]);
    view.dispose();
  });

  it("approves the open stage from the viewer", async () => {
    const { host, view, calls } = await mount();
    host.querySelector("#approvestage").click();
    await flush();
    expect(calls).toContainEqual([
      "issue.stage_approve",
      { issue_id: "issue-1", stage_id: "s1", thread_limit: FIRST_PAGE_ITEMS },
    ]);
    view.dispose();
  });

  it("sends a doc comment as an anchored message on the issue's conversation", async () => {
    const { host, view, calls } = await mount();
    const docEl = host.querySelector("#stagedoc");
    const heading = docEl.querySelector("h1");
    // Stand in for the selection flow: the layer turns a selected passage into
    // a pending comment, and the tray sends it.
    const selection = {
      anchorNode: heading.firstChild,
      focusNode: heading.firstChild,
      toString: () => "Wire",
      getRangeAt: () => ({ getBoundingClientRect: () => ({ top: 0, bottom: 0, left: 0, right: 0 }) }),
      isCollapsed: false,
      rangeCount: 1,
    };
    vi.spyOn(window, "getSelection").mockReturnValue({ ...selection, removeAllRanges: () => {} });
    document.dispatchEvent(new Event("selectionchange"));
    await new Promise((resolve) => setTimeout(resolve, 400));
    document.querySelector(".comment-pop .cp-add").click();
    document.querySelector(".comment-pop .cp-input").value = "why items[]?";
    document.querySelector(".comment-pop .cp-save").click();
    await flush();
    expect(host.querySelector(".pcomment")).toBeTruthy();
    host.querySelector(".cssend").click();
    await flush();
    const posted = calls.find(([method]) => method === "issue.comment_add");
    expect(posted[1].issue_id).toBe("issue-1");
    expect(posted[1].stage_id).toBe("s1");
    expect(posted[1].body).toBe("why items[]?");
    expect(posted[1].anchor.heading_path).toEqual(["Wire"]);
    expect(posted[1].anchor.line_start).toBe(1);
    view.dispose();
  });

  // A drag is a comment that has not been said yet: the popover opens only once
  // the handles settle, so until then nothing but the selection itself knows the
  // reader is holding a passage of this doc.
  it("leaves the doc alone while a passage is being selected on it", async () => {
    const stages = [stage()];
    const { host, view } = await mount({ stages, pollMs: 10 });
    const heading = host.querySelector("#stagedoc h1");
    vi.spyOn(window, "getSelection").mockReturnValue({
      anchorNode: heading.firstChild,
      focusNode: heading.firstChild,
      toString: () => "Wire",
      isCollapsed: false,
      rangeCount: 1,
      removeAllRanges: () => {},
      getRangeAt: () => ({ getBoundingClientRect: () => ({ top: 0, bottom: 0, left: 0, right: 0 }) }),
    });

    stages[0].title = "Rewired underneath";
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(host.contains(heading), "the doc the selection points into was replaced").toBe(true);
    expect(host.textContent).not.toContain("Rewired underneath");

    // Letting go hands the surface back: the next pass draws what moved.
    vi.restoreAllMocks();
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(host.textContent).toContain("Rewired underneath");
    view.dispose();
  });

  it("hangs the comments already on the doc in its margin, and withdraws one by its message id", async () => {
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
    host.querySelector(".commentcard .cc-x").click();
    await flush();
    expect(calls).toContainEqual(["issue.comment_delete", { issue_id: "issue-1", comment_id: "message-7" }]);
    view.dispose();
  });

  it("dispatches Implement All with the assignment's overrides", async () => {
    const ready = issuePayload({ state: "approved", stages: [{ id: "s1", state: "approved" }] });
    const { host, view, calls } = await mount({
      issue: ready,
      stages: [stage({ state: "approved", approval: "approved" })],
      loadCatalog: async () => ({ providers: [{ id: "claude", label: "Claude Code", models: [], efforts: ["high"] }] }),
    });
    host.querySelector("#assigntoggle").click();
    await flush();
    const base = document.querySelector(".assign-pop #assignbase");
    base.value = "release";
    base.dispatchEvent(new Event("input"));
    host.querySelector("#implementall").click();
    await flush();
    // Dispatch is a decisive gate: the modal outlines what happens, and taking
    // it is what sends the verb.
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    await flush();
    const dispatched = calls.find(([method]) => method === "issue.implement_all");
    expect(dispatched[1]).toEqual({ issue_id: "issue-1", base_branch: "release", thread_limit: FIRST_PAGE_ITEMS });
    view.dispose();
  });

  it("offers either checkout, and refuses an existing agent because implementation is a handoff", async () => {
    const { host, view } = await mount();
    host.querySelector("#assigntoggle").click();
    await flush();
    const panel = document.querySelector(".assign-pop");
    const worktree = panel.querySelector("#assignworktree");
    expect([...worktree.options].map((option) => option.value)).toEqual(["new", "existing"]);
    expect(worktree.querySelector('option[value="existing"]').disabled).toBe(false);
    expect(panel.querySelector("#assignagent").querySelector('option[value="existing"]').disabled).toBe(true);
    expect(panel.querySelector(".ivassign-gap").textContent).toMatch(/fresh agent/i);
    view.dispose();
  });

  it("keeps the assignment out of the rail: a control that opens it, never the fields", async () => {
    const { host, view } = await mount();
    const rail = host.querySelector(".ivstages");
    const rowsBefore = rail.querySelectorAll(".ivassign > *").length;
    host.querySelector("#assigntoggle").click();
    await flush();
    // Open, and the rail says so — but the rail has not grown by a single node.
    expect(host.querySelector("#assigntoggle").getAttribute("aria-expanded")).toBe("true");
    expect(rail.querySelectorAll(".ivassign > *")).toHaveLength(rowsBefore);
    expect(rail.querySelector("select")).toBeNull();
    expect(rail.querySelector("#assignbase")).toBeNull();
    expect(document.querySelector(".assign-pop #assignbase")).toBeTruthy();
    view.dispose();
  });

  it("shuts the overlay on Done, and says so back in the rail", async () => {
    const { host, view } = await mount();
    host.querySelector("#assigntoggle").click();
    await flush();
    document.querySelector(".assign-pop [data-assign-close]").click();
    await flush();
    expect(document.querySelector(".assign-pop")).toBeNull();
    expect(host.querySelector("#assigntoggle").getAttribute("aria-expanded")).toBe("false");
    view.dispose();
  });

  it("takes the overlay with it when the surface goes away", async () => {
    const { host, view } = await mount();
    host.querySelector("#assigntoggle").click();
    await flush();
    expect(document.querySelector(".assign-pop")).toBeTruthy();
    view.dispose();
    expect(document.querySelector(".assign-pop")).toBeNull();
  });

  it("holds a choice made in the overlay across a poll pass", async () => {
    const { host, view } = await mount({ pollMs: 5 });
    host.querySelector("#assigntoggle").click();
    await flush();
    const worktree = document.querySelector(".assign-pop #assignworktree");
    worktree.value = "existing";
    worktree.dispatchEvent(new Event("change"));
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await flush();
    expect(document.querySelector(".assign-pop #assignworktree").value).toBe("existing");
    expect(host.querySelector(".ivassign-sum").textContent).toMatch(/existing worktree/i);
    view.dispose();
  });

  it("dispatches Implement All into the branch the reviewer picked", async () => {
    const ready = issuePayload({ state: "approved", stages: [{ id: "s1", state: "approved" }] });
    const { host, view, calls } = await mount({
      issue: ready,
      stages: [stage({ state: "approved", approval: "approved" })],
      loadWorkItems: async () => [
        { kind: "branch", project_id: "proj-1", branch: "feature-x", worktree_id: "wt-1" },
        { kind: "branch", project_id: "proj-1", branch: "main", worktree_id: "wt-main", primary: true },
      ],
    });
    host.querySelector("#assigntoggle").click();
    await flush();
    const target = document.querySelector(".assign-pop #assignworktree");
    target.value = "existing";
    target.dispatchEvent(new Event("change"));
    await flush();
    const branch = document.querySelector(".assign-pop #assignworktreeid");
    // The primary checkout is the repository, not a worktree to hand over.
    expect([...branch.options].map((option) => option.value)).toEqual(["", "wt-1"]);
    branch.value = "wt-1";
    branch.dispatchEvent(new Event("change"));
    await flush();
    host.querySelector("#implementall").click();
    await flush();
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    await flush();
    const dispatched = calls.find(([method]) => method === "issue.implement_all");
    expect(dispatched[1]).toEqual({ issue_id: "issue-1", worktree_id: "wt-1", thread_limit: FIRST_PAGE_ITEMS });
    view.dispose();
  });

  it("renders the implementation lineage under the stages and opens a branch's changes", async () => {
    const routed = [];
    const { host, view } = await mount({
      issue: issuePayload({
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

  it("reads the plan of an issue that has no stages at all", async () => {
    const { host, view } = await mount({ issue: issuePayload({ stages: [] }), stages: [] });
    expect(host.querySelector(".ivviewer").textContent).toContain("The whole plan");
    expect(host.querySelector("#stagelist").textContent).toContain("No stages yet");
    view.dispose();
  });

  it("says the agent is still drafting rather than offering a plan that does not exist", async () => {
    const { host, view, calls } = await mount({ issue: issuePayload({ state: "drafting", stages: [] }), stages: [] });
    expect(host.querySelector(".ivviewer").textContent).toContain("drafting the plan");
    expect(calls.some(([method]) => method === "issue.stage_doc")).toBe(false);
    view.dispose();
  });

  it("does not claim an agent is drafting an issue nothing has started on", async () => {
    // A created issue is inert: the router files it and nothing runs until the
    // first message. Saying an agent is drafting would put a worker on the
    // surface that does not exist.
    const { host, view, calls } = await mount({ issue: issuePayload({ state: "created", stages: [] }), stages: [] });
    const viewer = host.querySelector(".ivviewer").textContent;
    expect(viewer).not.toContain("drafting the plan");
    expect(viewer).toContain("first message");
    expect(calls.some(([method]) => method === "issue.stage_doc")).toBe(false);
    view.dispose();
  });

  it("latches a deleted issue instead of repainting over it", async () => {
    const gone = [];
    const { host, view, calls } = await mount({
      fail: { "issue.get": "unknown issue_id: issue-1" },
      onGone: () => gone.push(true),
    });
    expect(host.textContent).toContain("This Issue no longer exists");
    const fetched = calls.filter(([method]) => method === "issue.get").length;
    await flush();
    expect(calls.filter(([method]) => method === "issue.get").length).toBe(fetched);
    host.querySelector("#goneback").click();
    expect(gone).toEqual([true]);
    view.dispose();
  });

  it("names a bound on every issue.get, so no read of it ships a conversation whole", async () => {
    // The only thing this surface reads off the payload's thread is the newest
    // sequence — the rail beside it owns what gets rendered. A read that names
    // no bound gets every item the conversation ever held, over E2EE, to
    // compute one integer.
    const selection = createAgentSelection("agent:one");
    const { view, calls } = await mount({ agentSelection: selection });
    const reads = calls.filter(([method]) => method === "issue.get");
    expect(reads).toHaveLength(1);
    expect(reads[0][1].thread_limit).toBe(1);
    expect(reads[0][1].thread_after_sequence).toBeUndefined();
    view.dispose();
  });

  it("names a bound again on the read after a bubble switch, which drops the cursor", async () => {
    const selection = createAgentSelection("agent:one");
    const { view, calls } = await mount({ agentSelection: selection, pollMs: 1 });
    selection.set("agent:two");
    // The FIRST read of the new bubble is the one that owes a bound: every read
    // after it is riding a cursor into a conversation this view now holds.
    const afterSwitch = (await readsForAgent(calls, "agent:two"))[0][1];
    expect(calls.filter(([method]) => method === "issue.get").length).toBeGreaterThan(1);
    expect(afterSwitch.thread_limit).toBe(1);
    expect(afterSwitch.thread_after_sequence).toBeUndefined();
    view.dispose();
  });

  it("renders the doc's error state with a Retry that clears the latch", async () => {
    const { host, view, calls } = await mount({ fail: { "issue.stage_doc": "read failed" } });
    expect(host.querySelector("#stagedocretry")).toBeTruthy();
    const reads = calls.filter(([method]) => method === "issue.stage_doc").length;
    expect(reads).toBe(1); // latched off: the poll does not retry it
    view.dispose();
  });
});
