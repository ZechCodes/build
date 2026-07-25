// The plan-authoring sheet — the one way work enters Build. A plan is
// project-scoped: Build drafts it, you review it, then Implement mints the run
// (plan.create → the plan surface).
//
// There used to be a second path here ("Quick task") that skipped planning and
// dispatched a plan-less run straight into a worktree. It is gone: an unplanned
// coding session is now a claude/codex tab off the tab row's `+`, driven by the
// human who opened it rather than tracked as a run nobody planned.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go, loadModelCatalog } from "../app.js";
import { notifyError } from "../core/notify.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  modelParams,
  providerOptionsHtml,
} from "../core/modelPicker.js";

export async function openNewPlan({ projectId } = {}) {
  $("#sheet").innerHTML = `
    <h3>New plan</h3>
    <div class="sub">Author a plan at the project level. Build drafts it, you review, then Implement when you're ready.</div>
    <textarea id="goal" placeholder="e.g. Add a /health endpoint that returns build SHA and uptime…"></textarea>
    <div class="field"><label>Project</label><select id="project"><option>loading…</option></select></div>
    <div class="field-row" style="display:flex;gap:10px">
      <div class="field" style="flex:1"><label>Agent</label><select id="provider"><option value="claude">Claude Code</option></select></div>
      <div class="field" style="flex:1"><label>Model</label><select id="model"><option value="">Harness default</option></select></div>
      <div class="field" style="flex:1"><label>Reasoning effort</label><select id="effort"><option value="">Default effort</option></select></div>
    </div>
    <div class="yolo">Agents run on your machine in YOLO mode (no sandbox). A worktree isolates the branch, not the machine.</div>
    <div class="adderr" id="ntkerr"></div>
    <div class="row"><span class="dim mono" id="agentname" style="font-size:11px">${esc("Claude Code")}</span>
      <button class="btn" id="cancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="dispatch">Create plan</button></div>`;
  $("#scrim").classList.add("show");
  $("#goal").focus();
  $("#cancel").onclick = () => $("#scrim").classList.remove("show");
  // Guard against dispatching before we know the projects: keep Create disabled
  // until project.list resolves with at least one project (re-enabled below).
  $("#dispatch").disabled = true;
  $("#goal").oninput = () => ($("#ntkerr").textContent = "");

  // Populate the project picker; the bridge picks the first if none chosen.
  const select = $("#project");
  try {
    const { projects } = await App.call("project.list");
    select.innerHTML = projects.length
      ? projects.map((p) => `<option value="${esc(p.project_id)}">${esc(p.name)} · ${esc(p.base_branch)}</option>`).join("")
      : '<option value="">(no projects — add one in Settings)</option>';
    if (projectId && projects.some((p) => p.project_id === projectId)) select.value = projectId;
    // Only enable Create once we actually have a project to dispatch into; the
    // zero-project placeholder keeps it disabled and explains why.
    if (projects.length) $("#dispatch").disabled = false;
  } catch {
    select.innerHTML = '<option value="">(could not load projects)</option>';
  }

  // Model + effort selectors from the bridge's catalog (never hardcoded here).
  let catalog = { default_provider: "claude", providers: [] };
  try {
    catalog = await loadModelCatalog();
    $("#provider").innerHTML = providerOptionsHtml(catalog.providers, catalog.default_provider);
  } catch {
    /* selectors keep only the defaults */
  }
  const syncModel = () => {
    const providerCatalog = catalogForProvider(catalog, $("#provider").value);
    const model = modelInCatalog(providerCatalog.models, $("#model").value);
    const selectedEffort = $("#effort").value;
    $("#effort").innerHTML = effortOptionsHtml(providerCatalog.efforts, selectedEffort, model);
    const supported = effortSupported(providerCatalog.models, $("#model").value);
    $("#effort").disabled = !supported;
    if (!supported) $("#effort").value = "";
  };
  const syncProvider = () => {
    const providerCatalog = catalogForProvider(catalog, $("#provider").value);
    $("#model").innerHTML = modelOptionsHtml(providerCatalog.models, "");
    $("#effort").innerHTML = effortOptionsHtml(providerCatalog.efforts, "");
    $("#agentname").textContent = providerCatalog.label || "Coding agent";
    syncModel();
  };
  $("#provider").onchange = syncProvider;
  $("#model").onchange = syncModel;
  syncProvider();

  $("#dispatch").onclick = async () => {
    const goal = $("#goal").value.trim();
    if (!goal) {
      $("#ntkerr").textContent = "Enter a goal first.";
      return;
    }
    // Optimistic close: capture everything, drop the sheet immediately, then
    // await the RPC. On failure the sheet is gone, so a persistent error
    // notification (not a resurrected button label) carries the reason.
    const params = {
      goal,
      project_id: select.value || undefined,
      ...modelParams(
        catalogForProvider(catalog, $("#provider").value).models,
        $("#model").value,
        $("#effort").value,
        $("#provider").value,
      ),
    };
    $("#scrim").classList.remove("show");
    try {
      const plan = await App.call("plan.create", params);
      go({ name: "plan", projectId: plan.project_id || params.project_id, id: plan.plan_id, tab: "review" });
    } catch (e) {
      notifyError("Couldn't create the plan", e.message);
    }
  };
}
