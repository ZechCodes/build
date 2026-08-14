// The branch work item's surface: two tabs, Changes and Files, and nothing
// else — the conversation is the agent rail and the terminals are the console.
//
// This is the shell of that surface. It reads the branch row from the bridge
// (`branch.get`, the redesigned wire) so the identity and status on screen are
// real, and hands each tab a mount point the pane items fill.
//
// A branch name and a title come from the repo and from agents: untrusted, and
// escaped everywhere they are painted.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { tabShellHtml } from "../core/tabshell.js";
import { mountConsoleRegion } from "../core/consoleRegion.js";
import "../styles/shell.css";

const BRANCH_TABS = [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
];

/** The branch's status, as the toolbar's right side reads it: working time and
 *  the diffstat. Pure. */
export function branchStatusHtml(row) {
  if (!row) return "";
  const stat = row.stat ? `<span class="sstat">${esc(row.stat)}</span>` : "";
  const state = row.state ? `<span class="sstate">${esc(row.state)}</span>` : "";
  return `${state}${stat}`;
}

export async function renderBranch() {
  const root = $("#root");
  const { projectId, branch } = App.route;
  const tab = App.route.tab || "changes";
  root.className = "surface";
  root.innerHTML = `
    <div class="surface-bar">
      <div class="tabrow" id="branch-tabs"></div>
      <div class="projectactions" id="branch-status"></div>
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

  const paintStatus = async () => {
    try {
      const view = await App.call("branch.get", { project_id: projectId, branch });
      const status = $("#branch-status");
      if (status) status.innerHTML = branchStatusHtml(view);
    } catch {
      /* the next tick retries; a stale status must not blank the surface */
    }
  };
  App.viewDispose = () => {
    const region = $("#console-region");
    if (region) region.innerHTML = "";
  };
  await paintStatus();
  App.poll = setInterval(paintStatus, 4000);
}
