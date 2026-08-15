// @vitest-environment jsdom
// The issue view as it behaves: two persistent columns, a stage selection that
// never costs the list, doc comments that go out as anchored conversation
// messages, and the assignment control feeding the implement verbs.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountIssueView, openingStageId, docAnnotatable } from "../src/core/issueView.js";

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
    expect(calls).toContainEqual(["issue.stage_approve", { issue_id: "issue-1", stage_id: "s1" }]);
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
    const base = host.querySelector("#assignbase");
    base.value = "release";
    base.dispatchEvent(new Event("input"));
    host.querySelector("#implementall").click();
    await flush();
    // Dispatch is a decisive gate: the modal outlines what happens, and taking
    // it is what sends the verb.
    document.querySelector("#confirm-scrim [data-confirm-ok]").click();
    await flush();
    const dispatched = calls.find(([method]) => method === "issue.implement_all");
    expect(dispatched[1]).toEqual({ issue_id: "issue-1", base_branch: "release" });
    view.dispose();
  });

  it("offers either checkout, and refuses an existing agent because implementation is a handoff", async () => {
    const { host, view } = await mount();
    host.querySelector("#assigntoggle").click();
    await flush();
    const worktree = host.querySelector("#assignworktree");
    expect([...worktree.options].map((option) => option.value)).toEqual(["new", "existing"]);
    expect(worktree.querySelector('option[value="existing"]').disabled).toBe(false);
    expect(host.querySelector("#assignagent").querySelector('option[value="existing"]').disabled).toBe(true);
    expect(host.querySelector(".ivassign-gap").textContent).toMatch(/fresh agent/i);
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
    const target = host.querySelector("#assignworktree");
    target.value = "existing";
    target.dispatchEvent(new Event("change"));
    await flush();
    const branch = host.querySelector("#assignworktreeid");
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
    expect(dispatched[1]).toEqual({ issue_id: "issue-1", worktree_id: "wt-1" });
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

  it("renders the doc's error state with a Retry that clears the latch", async () => {
    const { host, view, calls } = await mount({ fail: { "issue.stage_doc": "read failed" } });
    expect(host.querySelector("#stagedocretry")).toBeTruthy();
    const reads = calls.filter(([method]) => method === "issue.stage_doc").length;
    expect(reads).toBe(1); // latched off: the poll does not retry it
    view.dispose();
  });
});
