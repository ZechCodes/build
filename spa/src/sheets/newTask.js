// The new-task sheet: describe a goal, pick a project, dispatch.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";

export async function openNewTask() {
  $("#sheet").innerHTML = `
    <h3>New task</h3><div class="sub">Describe a goal. Build plans it, you review, an agent ships it.</div>
    <textarea id="goal" placeholder="e.g. Add a /health endpoint that returns build SHA and uptime…"></textarea>
    <div class="field"><label>Project</label><select id="project"><option>loading…</option></select></div>
    <label class="toggle" style="margin-top:12px"><input type="checkbox" id="quick"> Quick task — skip planning (small, unambiguous changes)</label>
    <div class="yolo">Agents run on your machine in YOLO mode (no sandbox). A worktree isolates the branch, not the machine.</div>
    <div class="row"><span class="dim mono" style="font-size:11px">${esc("Claude Code")}</span>
      <button class="btn" id="cancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="dispatch">Create</button></div>`;
  $("#scrim").classList.add("show");
  $("#goal").focus();
  $("#cancel").onclick = () => $("#scrim").classList.remove("show");

  // Populate the project picker; the bridge picks the first if none chosen.
  const select = $("#project");
  try {
    const { projects } = await App.call("project.list");
    select.innerHTML = projects.length
      ? projects.map((p) => `<option value="${esc(p.project_id)}">${esc(p.name)} · ${esc(p.base_branch)}</option>`).join("")
      : '<option value="">(no projects — add one in Settings)</option>';
  } catch {
    select.innerHTML = '<option value="">(could not load projects)</option>';
  }

  $("#dispatch").onclick = async () => {
    const goal = $("#goal").value.trim();
    if (!goal) return;
    const kind = $("#quick").checked ? "quick" : "standard";
    const project_id = select.value || undefined;
    $("#dispatch").disabled = true;
    $("#dispatch").textContent = "dispatching…";
    try {
      const t = await App.call("task.dispatch", { goal, kind, project_id });
      $("#scrim").classList.remove("show");
      go({ name: "task", id: t.task_id, tab: kind === "quick" ? "diff" : "plan" });
    } catch (e) {
      $("#dispatch").textContent = "error: " + e.message.slice(0, 40);
      $("#dispatch").disabled = false;
    }
  };
}
