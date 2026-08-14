// The branch work item's surface: two tabs, Changes and Files, and nothing
// else — the conversation is the agent rail and the terminals are the console.
//
// The row is just tabs now. Which branch this is, which project it lives in,
// how long its agent has been working and what the diff weighs are the
// toolbar's (core/toolbar.js), one row above; the surface below states only
// what is inside it.
//
// A branch name comes from the repo: untrusted, and escaped everywhere it is
// painted.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { tabShellHtml } from "../core/tabshell.js";
import { mountConsoleRegion } from "../core/consoleRegion.js";
import { mountAgentRail } from "../core/agentRail.js";
import "../styles/shell.css";

const BRANCH_TABS = [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
];

export async function renderBranch() {
  const root = $("#root");
  const { projectId, branch } = App.route;
  const tab = App.route.tab || "changes";
  root.className = "surface";
  root.innerHTML = `
    <div class="surface-bar">
      <div class="tabrow" id="branch-tabs"></div>
    </div>
    <div id="tabbody">
      <div class="shell-stub">
        <h2>${esc(branch)}</h2>
        <p>${tab === "files" ? "The file tree and viewer render here." : "The commit list and the stacked diffs render here."}</p>
      </div>
    </div>`;
  $("#branch-tabs").innerHTML = tabShellHtml({ tabs: BRANCH_TABS, active: tab });
  $("#branch-tabs")
    .querySelectorAll("[data-tab]")
    .forEach((cell) => {
      cell.onclick = () => go({ name: "branch", projectId, branch, tab: cell.dataset.tab });
    });
  mountConsoleRegion($("#console-region"));
  // The agents beside the work, not instead of it: the rail belongs to this
  // branch, so it is mounted with the surface and torn down with it.
  const rail = mountAgentRail($("#agent-rail"), { kind: "branch", projectId, branch });

  App.viewDispose = () => {
    rail.dispose();
    const region = $("#console-region");
    if (region) region.innerHTML = "";
  };
}
