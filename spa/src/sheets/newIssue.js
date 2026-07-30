// Filing an issue — the one way work enters Build. An issue is project-scoped
// and states what you want; Build drafts the plan to implement it, you discuss
// and approve that plan, then Implement mints the run whose diff you discuss and
// merge. On the wire it is still plan.create: the issue IS the plan record, and
// "issue" is what the whole thread of it is called.
//
// There used to be a second path here ("Quick task") that skipped planning and
// dispatched a plan-less run straight into a worktree. It is gone: an unplanned
// coding session is now an agent tab in a worktree, driven by the human who
// opened it rather than tracked as a run nobody filed.

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
import { loadAgentDefaults } from "../core/agentDefaults.js";

export async function openNewIssue({ projectId } = {}) {
  $("#sheet").innerHTML = `
    <h3>New issue</h3>
    <div class="sub">Say what you want. Build drafts ordered stage plans, you discuss and approve them, then implement when you're ready.</div>
    <textarea id="goal" placeholder="e.g. Add a /health endpoint that returns build SHA and uptime…"></textarea>
    <div class="advanced" id="advanced" hidden>
      <div class="field"><label>Harness</label>
        <div class="field-row" style="display:flex;gap:10px">
          <div class="field" style="flex:1"><label>Agent</label><select id="provider"><option value="claude">Claude Code</option></select></div>
          <div class="field" style="flex:1"><label>Model</label><select id="model"><option value="">Harness default</option></select></div>
          <div class="field" style="flex:1"><label>Reasoning effort</label><select id="effort"><option value="">Default effort</option></select></div>
        </div>
      </div>
      <div class="yolo">Agents run on your machine in YOLO mode (no sandbox). A worktree isolates the branch, not the machine.</div>
      <div class="dim" style="font-size:11.5px">Defaults come from your account settings.</div>
    </div>
    <div class="adderr" id="ntkerr"></div>
    <div class="row">
      <button class="btn mini advtoggle" id="advtoggle" type="button" aria-expanded="false"
        title="Agent, model and reasoning effort">${esc("Claude Code")} <span class="advcaret">▸</span></button>
      <button class="btn" id="cancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="dispatch">Create</button></div>`;
  $("#scrim").classList.add("show");
  $("#goal").focus();
  // Which harness will run is worth SEEING without opening anything; changing it
  // is rare, so it lives behind this button — and the button IS the harness name.
  // Opening retires it: the panel it summons says everything it said, so leaving
  // it behind would be two labels for one fact.
  const advanced = $("#advanced");
  const toggle = $("#advtoggle");
  toggle.onclick = () => {
    advanced.hidden = false;
    toggle.hidden = true;
  };
  $("#cancel").onclick = () => $("#scrim").classList.remove("show");
  $("#goal").oninput = () => ($("#ntkerr").textContent = "");

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
  const syncProvider = (keep = null) => {
    const providerCatalog = catalogForProvider(catalog, $("#provider").value);
    $("#model").innerHTML = modelOptionsHtml(providerCatalog.models, (keep && keep.model) || "");
    $("#effort").innerHTML = effortOptionsHtml(providerCatalog.efforts, (keep && keep.effort) || "");
    // The toggle carries the harness name — the one thing about this panel worth
    // knowing while it is shut.
    toggle.childNodes[0].nodeValue = `${providerCatalog.label || "Coding agent"} `;
    syncModel();
  };
  $("#provider").onchange = () => syncProvider();
  $("#model").onchange = syncModel;

  // Start from the account defaults, ignoring any the catalog no longer offers —
  // a pinned model the daemon has dropped must not silently pin nothing.
  const defaults = loadAgentDefaults();
  if (defaults.provider && catalog.providers.some((p) => p.id === defaults.provider)) {
    $("#provider").value = defaults.provider;
  }
  syncProvider(defaults);
  if (defaults.model) $("#model").value = modelInCatalog(catalogForProvider(catalog, $("#provider").value).models, defaults.model);
  syncModel();
  if (defaults.effort && !$("#effort").disabled) $("#effort").value = defaults.effort;

  $("#dispatch").onclick = async () => {
    const goal = $("#goal").value.trim();
    if (!goal) {
      $("#ntkerr").textContent = "Describe the issue first.";
      return;
    }
    // Optimistic close: capture everything, drop the sheet immediately, then
    // await the RPC. On failure the sheet is gone, so a persistent error
    // notification (not a resurrected button label) carries the reason.
    const params = {
      goal,
      // The FAB files against the project whose page you are on; the bridge falls
      // back to its default project if the route somehow carried none.
      project_id: projectId || undefined,
      ...modelParams(
        catalogForProvider(catalog, $("#provider").value).models,
        $("#model").value,
        $("#effort").value,
        $("#provider").value,
      ),
    };
    $("#scrim").classList.remove("show");
    try {
      const issue = await App.call("issue.create", params);
      go({
        name: "plan",
        projectId: issue.project_id || params.project_id,
        id: issue.issue_id || issue.plan_id,
        tab: "conversation",
      });
    } catch (e) {
      notifyError("Couldn't file the issue", e.message);
    }
  };
}
