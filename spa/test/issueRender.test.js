import { describe, it, expect } from "vitest";
import {
  DOCS_UNAVAILABLE,
  docErrorPaneHtml,
  stageRowHtml,
  stageListHtml,
  lineageHtml,
  assignmentHtml,
  assignmentPanelHtml,
  stageViewerHtml,
  stageNavHtml,
  docCommentCardHtml,
  docMarkerParts,
} from "../src/core/issueRender.js";
import { implementParams } from "../src/core/issueModel.js";

const issue = (overrides = {}) => ({
  issue_id: "issue-1",
  plan_id: "issue-1",
  goal: "Rebuild the issue view",
  state: "plan_review",
  project: "Build",
  base_branch: "main",
  ...overrides,
});

const stagesData = (stages) => ({ stages });

const stage = (overrides = {}) => ({ id: "s1", title: "Wire", state: "planned", ...overrides });

const assignment = { worktree: "new", agent: "new", base: "", provider: "claude", model: "", effort: "" };

const catalog = { providers: [{ id: "claude", label: "Claude Code", models: [], efforts: ["high"] }] };

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
  it("states where the issue stands, where it lives, and lists its stages", () => {
    const html = stageListHtml({
      issue: issue(),
      stagesData: stagesData([stage(), stage({ id: "s2", title: "Render" })]),
      selectedStageId: "s1",
      assignment,
    });
    expect(html).toContain("READY TO REVIEW");
    expect(html).toContain("Build");
    expect(html).toContain("main");
    expect(html).toContain('id="stagelist"');
    expect(html).toContain('data-stage="s2"');
  });

  // The issue's goal is its first message, and the conversation beside this rail
  // already carries it — repeating it above the stages is the same text twice.
  it("leaves the issue's message to the conversation", () => {
    const html = stageListHtml({
      issue: issue(),
      stagesData: stagesData([stage()]),
      selectedStageId: "s1",
      assignment,
    });
    expect(html).not.toContain("Rebuild the issue view");
    expect(html).not.toContain("ivtitle");
  });

  it("offers approve-all only while every stage is still planned", () => {
    const planned = stageListHtml({ issue: issue(), stagesData: stagesData([stage(), stage({ id: "s2" })]), assignment });
    expect(planned).toContain('id="approveall"');
    const mixed = stageListHtml({
      issue: issue(),
      stagesData: stagesData([stage(), stage({ id: "s2", state: "approved" })]),
      assignment,
    });
    expect(mixed).not.toContain('id="approveall"');
  });

  it("offers the issue gate at plan_review and the dispatch once the issue is ready", () => {
    const review = stageListHtml({ issue: issue({ state: "plan_review" }), stagesData: stagesData([stage()]), assignment });
    expect(review).toContain('id="approveissue"');
    const ready = stageListHtml({
      issue: issue({ state: "approved", stages: [{ id: "s1", state: "approved" }] }),
      stagesData: stagesData([stage({ state: "approved" })]),
      assignment,
    });
    expect(ready).toContain('id="implementall"');
  });

  it("says so rather than showing an empty list when there are no stages yet", () => {
    const html = stageListHtml({ issue: issue({ state: "drafting" }), stagesData: stagesData([]), assignment });
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

  it("renders nothing at all for an issue nobody has implemented", () => {
    expect(lineageHtml([])).toBe("");
    expect(lineageHtml(undefined)).toBe("");
  });
});

describe("assignmentHtml", () => {
  it("says the handoff in one line and nothing else", () => {
    const html = assignmentHtml({ assignment, open: false });
    expect(html).toContain("New worktree · New agent · claude");
    expect(html).not.toContain("<select");
  });

  it("stays that one line while the overlay it opens is open", () => {
    // The rail is a list of stages: the fields live in the overlay, so the row
    // only ever marks itself as the thing standing open.
    const html = assignmentHtml({ assignment, open: true });
    expect(html).not.toContain("<select");
    expect(html).not.toContain("<input");
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('aria-haspopup="dialog"');
  });
});

describe("assignmentPanelHtml", () => {
  it("opens onto both targets, offering an existing agent only to say why it cannot be used", () => {
    const html = assignmentPanelHtml({ assignment, catalog });
    expect(html).toContain('value="existing"');
    expect(html).toContain("disabled");
    expect(html).toMatch(/fresh agent/i);
  });

  it("offers a way out that is not a choice", () => {
    expect(assignmentPanelHtml({ assignment, catalog })).toContain("data-assign-close");
  });

  it("carries the base-branch and model choice the dispatch would use", () => {
    const html = assignmentPanelHtml({ assignment: { ...assignment, base: "release" }, catalog });
    expect(html).toContain('value="release"');
    expect(html).toContain("Claude Code");
  });

  it("swaps the base branch for a branch picker once an existing checkout is the target", () => {
    const html = assignmentPanelHtml({
      assignment: { ...assignment, worktree: "existing", worktreeId: "wt-1" },
      catalog,
      worktrees: [
        { id: "wt-1", label: "feature-x" },
        { id: "wt-2", label: "feature-y" },
      ],
    });
    expect(html).toContain('id="assignworktreeid"');
    expect(html).not.toContain('id="assignbase"');
    expect(html).toContain('value="wt-1" selected');
    expect(html).toContain("feature-y");
  });
});

// The panel paints an offer of two agents, and the dispatch it sits above has
// to send the one it painted: a select saying "Claude Code" that creates a TUI
// agent is the mismatch this pair exists to catch.
describe("the assignment's agent select and the dispatch under it", () => {
  const threeHarnesses = {
    default_provider: "claude_adk",
    providers: [
      {
        id: "claude_adk",
        label: "Claude Code",
        models: [{ id: "opus", label: "Opus", supports_effort: true }],
        efforts: ["high"],
      },
      { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
      { id: "codex", label: "Codex", models: [], efforts: [] },
    ],
  };
  const stale = { ...assignment, provider: "claude", model: "opus", effort: "high" };

  it("offers the two agents, painting a stale carrier as the one it clamps to", () => {
    const html = assignmentPanelHtml({ assignment: stale, catalog: threeHarnesses });
    expect(html).toContain('<option value="claude_adk" selected>Claude Code</option>');
    expect(html).toContain('<option value="codex">Codex</option>');
    expect(html).not.toContain("Claude Code TUI");
  });

  it("dispatches the agent the select painted, not the stale token behind it", () => {
    expect(implementParams("issue-1", stale, { catalog: threeHarnesses })).toEqual({
      issue_id: "issue-1",
      provider: "claude_adk",
      model: "opus",
      effort: "high",
    });
  });

  it("dispatches an untouched assignment with the agent the select displays", () => {
    expect(implementParams("issue-1", { ...assignment, provider: "" }, { catalog: threeHarnesses })).toEqual({
      issue_id: "issue-1",
      provider: "claude_adk",
    });
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
    expect(html).toContain("VALIDATED");
    expect(html).toContain('id="stagedoc"');
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

  it("steps to either neighbour, and says where in the issue the reader is", () => {
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
    expect(html).toContain('data-del="message-7"');
  });

  it("says where it points: the heading chain and the lines it was written on", () => {
    const html = docCommentCardHtml(comment({ anchor: { heading_path: ["Plan", "Schema"], snippet: "sqlite", line_start: 12, line_end: 18 } }));
    expect(html).toContain("Plan &gt; Schema:12-18");
  });

  it("falls back to the doc the bridge says it is on when no heading encloses it", () => {
    const html = docCommentCardHtml(comment({ anchor: { heading_path: [], snippet: "sqlite", line_start: 3, line_end: 3 } }));
    expect(html).toContain("docs/stage-1.md:3");
  });

  it("calls an unanchored comment general, and offers no withdraw once addressed", () => {
    const html = docCommentCardHtml(comment({ state: "addressed", agent_reply: "done", anchor: null }));
    expect(html).toContain("(general)");
    expect(html).toContain("cc-reply");
    expect(html).not.toContain("cc-x");
  });
});
