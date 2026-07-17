// The creation sheet, split into the two paths the plan/run model gives us
// (user-agency principle: name the two behaviours, don't hide one behind a
// checkbox). "New plan" authors a project-scoped plan you review, then
// Implement later (plan.create → the plan surface). "Quick task" skips planning
// and dispatches a plan-less run straight into a worktree (run.create → the run
// surface). A segmented switch chooses the path; `mode` sets the initial one.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go, loadModelCatalog } from "../app.js";
import { modelOptionsHtml, effortOptionsHtml, effortSupported, modelParams } from "../core/modelPicker.js";

export async function openNewTask({ projectId, mode = "plan" } = {}) {
  let path = mode === "quick" ? "quick" : "plan";

  $("#sheet").innerHTML = `
    <h3>New work</h3>
    <div class="segmented" id="modeswitch">
      <button class="btn seg" data-mode="plan" type="button">New plan</button>
      <button class="btn seg" data-mode="quick" type="button">Quick task</button>
    </div>
    <div class="sub" id="modesub"></div>
    <textarea id="goal" placeholder="e.g. Add a /health endpoint that returns build SHA and uptime…"></textarea>
    <div class="field"><label>Project</label><select id="project"><option>loading…</option></select></div>
    <div class="field-row" style="display:flex;gap:10px">
      <div class="field" style="flex:1"><label>Model</label><select id="model"><option value="">Harness default</option></select></div>
      <div class="field" style="flex:1"><label>Reasoning effort</label><select id="effort"><option value="">Default effort</option></select></div>
    </div>
    <div class="yolo">Agents run on your machine in YOLO mode (no sandbox). A worktree isolates the branch, not the machine.</div>
    <div class="row"><span class="dim mono" style="font-size:11px">${esc("Claude Code")}</span>
      <button class="btn" id="cancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="dispatch">Create</button></div>`;
  $("#scrim").classList.add("show");
  $("#goal").focus();
  $("#cancel").onclick = () => $("#scrim").classList.remove("show");

  // The segmented switch: the two paths are distinct verbs, so each has its own
  // explanation and its own dispatch. `path` drives the dispatch handler below.
  const applyMode = () => {
    $("#modeswitch")
      .querySelectorAll(".seg")
      .forEach((b) => b.classList.toggle("primary", b.dataset.mode === path));
    $("#modesub").textContent =
      path === "plan"
        ? "Author a plan at the project level. Build drafts it, you review, then Implement when you're ready."
        : "Skip planning for a small, unambiguous change. A worktree and coding agent start straight away.";
    $("#dispatch").textContent = path === "plan" ? "Create plan" : "Start task";
  };
  $("#modeswitch")
    .querySelectorAll(".seg")
    .forEach(
      (b) =>
        (b.onclick = () => {
          path = b.dataset.mode;
          applyMode();
        }),
    );
  applyMode();

  // Populate the project picker; the bridge picks the first if none chosen.
  const select = $("#project");
  try {
    const { projects } = await App.call("project.list");
    select.innerHTML = projects.length
      ? projects.map((p) => `<option value="${esc(p.project_id)}">${esc(p.name)} · ${esc(p.base_branch)}</option>`).join("")
      : '<option value="">(no projects — add one in Settings)</option>';
    if (projectId && projects.some((p) => p.project_id === projectId)) select.value = projectId;
  } catch {
    select.innerHTML = '<option value="">(could not load projects)</option>';
  }

  // Model + effort selectors from the bridge's catalog (never hardcoded here).
  let catalog = { models: [], efforts: [] };
  try {
    catalog = await loadModelCatalog();
    $("#model").innerHTML = modelOptionsHtml(catalog.models, "");
    $("#effort").innerHTML = effortOptionsHtml(catalog.efforts, "");
  } catch {
    /* selectors keep only the defaults */
  }
  $("#model").onchange = () => {
    const supported = effortSupported(catalog.models, $("#model").value);
    $("#effort").disabled = !supported;
    if (!supported) $("#effort").value = "";
  };

  $("#dispatch").onclick = async () => {
    const goal = $("#goal").value.trim();
    if (!goal) return;
    const project_id = select.value || undefined;
    const params = { goal, project_id, ...modelParams(catalog.models, $("#model").value, $("#effort").value) };
    $("#dispatch").disabled = true;
    $("#dispatch").textContent = "dispatching…";
    try {
      if (path === "quick") {
        const run = await App.call("run.create", params);
        $("#scrim").classList.remove("show");
        go({ name: "task", id: run.run_id, tab: "changes" });
      } else {
        const plan = await App.call("plan.create", params);
        $("#scrim").classList.remove("show");
        go({ name: "plan", id: plan.plan_id, tab: "review" });
      }
    } catch (e) {
      $("#dispatch").textContent = "error: " + e.message.slice(0, 40);
      $("#dispatch").disabled = false;
    }
  };
}
