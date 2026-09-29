import { describe, it, expect } from "vitest";
import {
  DOCS_UNAVAILABLE,
  docErrorPaneHtml,
  stageRowHtml,
  stageListHtml,
  lineageHtml,
  stageViewerHtml,
  stageNavHtml,
  docCommentCardHtml,
  docMarkerParts,
} from "../src/core/taskRender.js";

const task = (overrides = {}) => ({
  task_id: "task-1",
  plan_id: "task-1",
  goal: "Rebuild the task view",
  state: "plan_review",
  project: "Build",
  base_branch: "main",
  ...overrides,
});

const stagesData = (stages) => ({ stages });

const stage = (overrides = {}) => ({ id: "s1", title: "Wire", state: "planned", ...overrides });

describe("stageRowHtml", () => {
  it("numbers the stage, states where it is, and carries its open-comment count", () => {
    const html = stageRowHtml(stage({ approval: "approved", execution: "building", open_comments: 2 }), { index: 0 });
    expect(html).toContain('data-stage="s1"');
    expect(html).toContain("01");
    expect(html).toContain("BUILDING");
    expect(html).toContain("2 💬");
  });

  it("marks the open stage so the list says which one the viewer is showing", () => {
    expect(stageRowHtml(stage(), { index: 0, selected: true })).toContain("sel");
    expect(stageRowHtml(stage(), { index: 0, selected: false })).not.toContain('class="stagerow sel"');
  });

  it("escapes a title that carries markup", () => {
    expect(stageRowHtml(stage({ title: "<img src=x>" }), { index: 0 })).not.toContain("<img");
  });
});

describe("stageListHtml", () => {
  it("states where the task stands, where it lives, and lists its stages", () => {
    const html = stageListHtml({
      task: task(),
      stagesData: stagesData([stage(), stage({ id: "s2", title: "Render" })]),
      selectedStageId: "s1",
    });
    expect(html).toContain("READY TO REVIEW");
    expect(html).toContain("Build");
    expect(html).toContain("main");
    expect(html).toContain('id="stagelist"');
    expect(html).toContain('data-stage="s2"');
  });

  // The task's goal is its first message, and the conversation beside this rail
  // already carries it — repeating it above the stages is the same text twice.
  it("leaves the task's message to the conversation", () => {
    const html = stageListHtml({
      task: task(),
      stagesData: stagesData([stage()]),
      selectedStageId: "s1",
    });
    expect(html).not.toContain("Rebuild the task view");
    expect(html).not.toContain("ivtitle");
  });

  // Plans are history, and the bridge refuses every verb that would move one:
  // the head says where the plan stands and offers nothing to do about it.
  it("offers no gate, dispatch, delete or assignment in any state", () => {
    for (const state of ["plan_review", "approved", "abandoned"]) {
      const html = stageListHtml({ task: task({ state }), stagesData: stagesData([stage(), stage({ id: "s2" })]) });
      for (const id of ["approveall", "approvetask", "implementall", "taskdelete", "assigntoggle"]) {
        expect(html).not.toContain(`id="${id}"`);
      }
      expect(html).not.toContain("ivassign");
    }
  });

  it("says so rather than showing an empty list when there are no stages yet", () => {
    const html = stageListHtml({ task: task({ state: "drafting" }), stagesData: stagesData([]) });
    expect(html).toContain("No stages yet");
  });
});

describe("lineageHtml", () => {
  it("lists each implementation with its state and branch", () => {
    const html = lineageHtml([
      { run_id: "run-1", state: "merged", branch: "build/one" },
      { run_id: "run-2", state: "building", branch: "build/two" },
    ]);
    expect(html).toContain('data-run="run-1"');
    expect(html).toContain("build/two");
    expect(html).toContain("MERGED");
  });

  it("renders nothing at all for a task nobody has implemented", () => {
    expect(lineageHtml([])).toBe("");
    expect(lineageHtml(undefined)).toBe("");
  });
});

describe("stageViewerHtml", () => {
  it("heads the doc with the stage's title and state, and renders the doc", () => {
    const html = stageViewerHtml({
      stage: stage({ approval: "approved", execution: "complete" }),
      docHtml: "<h1>Wire</h1>",
      paneState: "ready",
    });
    expect(html).toContain("Wire");
    expect(html).toContain("COMPLETE");
    expect(html).not.toContain("VALIDATED");
    expect(html).toContain('id="stagedoc"');
  });

  it("says why a stage is incomplete in a neutral invalidation box, not a validation verdict", () => {
    const html = stageViewerHtml({
      stage: stage({ approval: "approved", execution: "incomplete", invalidation_reason: "worktree <moved>" }),
      paneState: "ready",
    });
    expect(html).toContain('<div class="stage-invalidation"><strong>Stage incomplete</strong>');
    expect(html).toContain("worktree &lt;moved&gt;");
    expect(html).not.toContain("stage-validation");
  });

  it("says what to do rather than showing a blank column when no stage is open", () => {
    expect(stageViewerHtml({ stage: null })).toContain("Pick a stage");
  });

  it("carries the doc's own error and unavailable states", () => {
    expect(stageViewerHtml({ stage: stage(), docHtml: docErrorPaneHtml("stage"), paneState: "error" })).toContain("stagedocretry");
    expect(DOCS_UNAVAILABLE).toMatch(/unavailable/i);
  });

  it("puts the stage steps on the viewer's own sticky bar, where a phone can reach them", () => {
    const html = stageViewerHtml({
      stage: stage({ id: "s2" }),
      stages: [stage({ id: "s1" }), stage({ id: "s2" })],
      docHtml: "<p>x</p>",
      paneState: "ready",
    });
    expect(html).toMatch(/<div class="actionbar">\s*<div class="stagenav"/);
    expect(html).toContain('data-stage-step="prev"');
  });

  it("shows the comments already on the doc, newest state first", () => {
    const html = stageViewerHtml({
      stage: stage(),
      docHtml: "<p>x</p>",
      paneState: "ready",
      comments: [
        { id: "message-2", body: "done", state: "addressed", agent_reply: "fixed", anchor: { heading_path: ["Wire"], snippet: "x" } },
        { id: "message-1", body: "why", state: "open", anchor: { heading_path: ["Wire"], snippet: "x" } },
      ],
    });
    expect(html.indexOf("message-1")).toBeLessThan(html.indexOf("message-2"));
  });
});

describe("stageNavHtml", () => {
  const stages = [stage({ id: "a", title: "Wire" }), stage({ id: "b", title: "Render" }), stage({ id: "c", title: "Ship" })];

  it("steps to either neighbour, and says where in the task the reader is", () => {
    const html = stageNavHtml({ stages, selectedStageId: "b" });
    expect(html).toContain('data-stage-step="prev"');
    expect(html).toContain('data-stage="a"');
    expect(html).toContain('data-stage-step="next"');
    expect(html).toContain('data-stage="c"');
    expect(html).toContain("2 / 3");
    expect(html).not.toContain("disabled");
  });

  it("names the stage each step would open", () => {
    const html = stageNavHtml({ stages, selectedStageId: "b" });
    expect(html).toContain('title="Wire"');
    expect(html).toContain('title="Ship"');
  });

  it("runs out at the ends rather than wrapping", () => {
    const first = stageNavHtml({ stages, selectedStageId: "a" });
    expect(first).toMatch(/data-stage-step="prev"[^>]*disabled/);
    expect(first).not.toMatch(/data-stage-step="next"[^>]*disabled/);
    const last = stageNavHtml({ stages, selectedStageId: "c" });
    expect(last).toMatch(/data-stage-step="next"[^>]*disabled/);
    expect(last).not.toMatch(/data-stage-step="prev"[^>]*disabled/);
  });

  it("offers nothing where there is nowhere to walk", () => {
    expect(stageNavHtml({ stages: [stage({ id: "a" })], selectedStageId: "a" })).toBe("");
    expect(stageNavHtml({ stages, selectedStageId: null })).toBe("");
    expect(stageNavHtml({})).toBe("");
  });
});

describe("docMarkerParts", () => {
  it("marks a heading with what is waiting on it", () => {
    const parts = docMarkerParts({ key: "wire", headingPath: ["Wire"], comments: [{ id: "message-1" }], open: 1, total: 1 });
    expect(parts.className).toBe("docmarker");
    expect(parts.label).toContain("1");
    expect(parts.title).toBe("1 comment");
  });

  it("reads as settled once every comment on it is addressed", () => {
    const parts = docMarkerParts({ key: "wire", headingPath: ["Wire"], comments: [{ id: "m" }], open: 0, total: 2 });
    expect(parts.className).toContain("addressed");
    expect(parts.title).toContain("addressed");
  });
});

describe("docCommentCardHtml", () => {
  const comment = (overrides = {}) => ({
    id: "message-7",
    path: "docs/stage-1.md",
    body: "be concrete",
    state: "open",
    ...overrides,
  });

  it("is addressed by the id of the message it IS", () => {
    const html = docCommentCardHtml(comment({ anchor: { heading_path: ["Plan"], snippet: "x", line_start: 4, line_end: 6 } }));
    expect(html).toContain('data-id="message-7"');
  });

  it("offers no withdraw on an open comment: the page is a record", () => {
    expect(docCommentCardHtml(comment({ anchor: null }))).not.toContain("cc-x");
  });

  it("says where it points: the heading chain and the lines it was written on", () => {
    const html = docCommentCardHtml(comment({ anchor: { heading_path: ["Plan", "Schema"], snippet: "sqlite", line_start: 12, line_end: 18 } }));
    expect(html).toContain("Plan &gt; Schema:12-18");
  });

  it("falls back to the doc the bridge says it is on when no heading encloses it", () => {
    const html = docCommentCardHtml(comment({ anchor: { heading_path: [], snippet: "sqlite", line_start: 3, line_end: 3 } }));
    expect(html).toContain("docs/stage-1.md:3");
  });

  it("calls an unanchored comment general, and carries the agent's reply once addressed", () => {
    const html = docCommentCardHtml(comment({ state: "addressed", agent_reply: "done", anchor: null }));
    expect(html).toContain("(general)");
    expect(html).toContain("cc-reply");
  });
});
