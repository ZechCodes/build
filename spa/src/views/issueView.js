// The issue work item's surface: two persistent columns, the planned stages and
// the stage viewer. No tabs — the conversation is the agent rail.
//
// This is the shell of that surface: it reads the issue and its stages from the
// bridge so the columns are real, and leaves the stage document, the comments
// and the worktree/agent assignment control to the items that fill them.
//
// Stage titles are written by agents: untrusted, escaped.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { mountConsoleRegion } from "../core/consoleRegion.js";
import { mountAgentRail } from "../core/agentRail.js";
import "../styles/shell.css";

/** Pure: the stage column. `selected` is a stage id, or null for none. */
export function stageColumnHtml(stages, selected) {
  if (!stages || !stages.length) return `<div class="sempty dim">No stages planned yet.</div>`;
  return stages
    .map(
      (stage) =>
        `<div class="srow${stage.id === selected ? " active" : ""}" data-stage="${esc(stage.id)}">` +
        `<span class="stitle">${esc(stage.title || stage.id)}</span>` +
        `<span class="sstate">${esc(stage.state || "")}</span></div>`,
    )
    .join("");
}

/** The issue's stages in the shape the column reads, from whatever the bridge
 *  answered. Pure. */
export function stagesFrom(payload) {
  return (payload?.stages || []).map((stage) => ({
    id: stage.stage_id || stage.id,
    title: stage.title || stage.name,
    state: stage.state,
  }));
}

export async function renderIssue() {
  const root = $("#root");
  const { projectId, id } = App.route;
  root.className = "surface";
  root.innerHTML = `
    <div id="tabbody" class="flush">
      <div class="issue-cols">
        <div class="issue-stages" id="issue-stages"><div class="sempty dim">Loading…</div></div>
        <div class="issue-stage-view" id="issue-stage-view">
          <!-- TODO(integration): the stage document and its comments are the
               plan track's; this is the seam they mount into. -->
          <div class="shell-stub"><h2>Stage</h2><p>The stage document and its comments render here.</p></div>
        </div>
      </div>
    </div>`;
  mountConsoleRegion($("#console-region"));
  // An issue carries exactly one agent session, and this is where you talk to
  // it — including the first message, which is what starts it.
  const rail = mountAgentRail($("#agent-rail"), { kind: "issue", projectId, issueId: id });

  const paint = async () => {
    let stages = [];
    try {
      stages = stagesFrom(await App.call("issue.stages", { issue_id: id }));
    } catch {
      // An issue with nothing planned has no stages to answer with — which a
      // freshly filed one never does, since its planning agent starts on the
      // first message. That is an empty column, not a column still loading.
      stages = [];
    }
    const column = $("#issue-stages");
    if (!column) return;
    column.innerHTML = stageColumnHtml(stages, App.route.stage || null);
    column.querySelectorAll("[data-stage]").forEach((row) => {
      row.onclick = () => go({ name: "issue", projectId, id, stage: row.dataset.stage });
    });
  };
  App.viewDispose = () => {
    rail.dispose();
    const region = $("#console-region");
    if (region) region.innerHTML = "";
  };
  await paint();
  App.poll = setInterval(paint, 4000);
}
