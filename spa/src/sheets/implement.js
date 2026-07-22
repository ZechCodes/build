// Implement-with-overrides: the plan cockpit's Implement split-button opens this
// when the user wants a base-branch or model different from the plan's defaults.
// Returns a promise that resolves with the created run (the caller navigates to
// it) or rejects on cancel (the split-button restores itself). A dispatch error
// keeps the sheet open so the user can adjust and retry.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  modelParams,
  normalizeModelCatalog,
  providerOptionsHtml,
} from "../core/modelPicker.js";

export function openImplementOptions(plan, catalog) {
  const fullCatalog = normalizeModelCatalog(catalog || {});
  const providers = fullCatalog.providers;
  const initialProvider = plan.provider || fullCatalog.default_provider || "claude";
  const initialCatalog = catalogForProvider(fullCatalog, initialProvider);
  return new Promise((resolve, reject) => {
    $("#sheet").innerHTML = `
      <h3>Implement with options</h3>
      <div class="sub">Create a run for “${esc(plan.goal)}”, overriding the base branch or model.</div>
      <div class="field"><label>Base branch</label>
        <input id="implbase" placeholder="${esc(plan.base_branch || "the plan's base branch")}"></div>
      <div class="field-row" style="display:flex;gap:10px">
        <div class="field" style="flex:1"><label>Agent</label><select id="implprovider">${providerOptionsHtml(providers, initialProvider)}</select></div>
        <div class="field" style="flex:1"><label>Model</label><select id="implmodel">${modelOptionsHtml(initialCatalog.models, plan.model || "")}</select></div>
        <div class="field" style="flex:1"><label>Reasoning effort</label><select id="impleffort">${effortOptionsHtml(initialCatalog.efforts, plan.effort || "", modelInCatalog(initialCatalog.models, plan.model))}</select></div>
      </div>
      <div class="row"><button class="btn" id="implcancel" style="margin-left:auto">Cancel</button>
        <button class="btn primary" id="implstart">Implement</button></div>
      <div class="adderr" id="implerr"></div>`;
    $("#scrim").classList.add("show");

    const providerSel = $("#implprovider");
    const modelSel = $("#implmodel");
    const effortSel = $("#impleffort");
    const syncEffort = () => {
      const providerCatalog = catalogForProvider(fullCatalog, providerSel.value);
      const selected = effortSel.value;
      const model = modelInCatalog(providerCatalog.models, modelSel.value);
      effortSel.innerHTML = effortOptionsHtml(providerCatalog.efforts, selected, model);
      const supported = effortSupported(providerCatalog.models, modelSel.value);
      effortSel.disabled = !supported;
      if (!supported) effortSel.value = "";
    };
    providerSel.onchange = () => {
      const providerCatalog = catalogForProvider(fullCatalog, providerSel.value);
      modelSel.innerHTML = modelOptionsHtml(providerCatalog.models, "");
      effortSel.innerHTML = effortOptionsHtml(providerCatalog.efforts, "");
      syncEffort();
    };
    modelSel.onchange = syncEffort;
    syncEffort();

    const close = () => $("#scrim").classList.remove("show");
    $("#implcancel").onclick = () => {
      close();
      reject(new Error("cancelled"));
    };
    $("#implstart").onclick = async () => {
      const base = $("#implbase").value.trim();
      const params = {
        plan_id: plan.plan_id,
        ...(base ? { base_branch: base } : {}),
        ...modelParams(
          catalogForProvider(fullCatalog, providerSel.value).models,
          modelSel.value,
          effortSel.value,
          providerSel.value,
        ),
      };
      $("#implstart").disabled = true;
      $("#implstart").textContent = "starting…";
      try {
        const run = await App.call("run.create", params);
        close();
        resolve(run);
      } catch (e) {
        $("#implstart").disabled = false;
        $("#implstart").textContent = "Implement";
        $("#implerr").textContent = e.message;
      }
    };
  });
}
