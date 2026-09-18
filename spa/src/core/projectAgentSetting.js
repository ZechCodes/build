// The device's project agent: the harness, model and reasoning effort a
// project's agent starts on.
//
// A project agent talks ABOUT a project rather than working in a checkout, and
// the harness that suits that job is often not the one coding work leads with.
// So it is asked for alone — and asked of the MACHINE, beside the fallback
// agent (core/defaultHarness.js), because the setting is persistent and the
// bridge that runs the agent is what holds it. No browser is asked at first use.
//
// Painted from settings.get and models.list, saved with settings.set, repainted
// from whatever the bridge answers: the controls show what the device actually
// holds, never what was merely attempted.

import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  providerOptionsHtml,
} from "./modelPicker.js";

const word = (value) => (typeof value === "string" ? value : "");

/** What a new project agent starts on, as a settings payload states it: the
 *  device's own words, with its fallback harness standing where they name no
 *  harness. An empty model or effort is "whatever the harness's own config
 *  says" — the same "no preference" the bridge means by leaving it out.
 *
 *  Never throws: a machine answering something this client cannot read has no
 *  preference as far as the reader is concerned. */
export function projectAgentChoiceOf(settings) {
  const chosen = settings?.project_agent || {};
  return {
    provider: word(chosen.provider) || word(settings?.default_harness),
    model: word(chosen.model),
    effort: word(chosen.effort),
  };
}

/** The panel, empty. `mountProjectAgentSetting` fills the selects from the
 *  bridge — rendering them pre-filled would show a choice nobody has confirmed
 *  is this machine's. */
export function projectAgentPanelHtml() {
  return `<div class="panel">
      <h3>🧭 Project agent</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">What a project's agent starts on. It is the one agent that talks about a project instead of working in a checkout, so it starts on its own choice rather than the fallback above. Agents already running keep what they were started on.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:150px"><label for="projectagentharness">Agent</label>
          <select id="projectagentharness" disabled><option>loading…</option></select></div>
        <div class="field" style="flex:1;min-width:150px"><label for="projectagentmodel">Model</label>
          <select id="projectagentmodel" disabled><option>loading…</option></select></div>
        <div class="field" style="flex:1;min-width:150px"><label for="projectagenteffort">Reasoning effort</label>
          <select id="projectagenteffort" disabled><option>loading…</option></select></div>
      </div>
      <div class="dim" id="projectagentsaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="projectagenterr"></div>
    </div>`;
}

/** The harness this machine's answer names, held to the catalog the same
 *  machine offers: a provider the catalog does not carry is a Settings error,
 *  not a silent move to another one. */
function offeredHarness(providers, providerId) {
  if (!providers.some((provider) => provider.id === providerId)) {
    throw new Error(
      "the project agent's harness is missing, malformed, or absent from models.list.providers",
    );
  }
  return providerId;
}

/** Wire the panel to the bridge: paint from settings.get, save on every change,
 *  and repaint from the answer. */
export async function mountProjectAgentSetting(host, { callRpc, onSaved = async () => {} }) {
  const harness = host.querySelector("#projectagentharness");
  const model = host.querySelector("#projectagentmodel");
  const effort = host.querySelector("#projectagenteffort");
  const saved = host.querySelector("#projectagentsaved");
  const error = host.querySelector("#projectagenterr");
  if (!harness || !model || !effort) return;
  const controls = [harness, model, effort];
  let providers = [];

  const clearUnconfirmedSelection = () => {
    for (const control of controls) {
      control.innerHTML = "";
      control.disabled = true;
    }
  };
  const renderConfirmedSettings = (settings) => {
    const choice = projectAgentChoiceOf(settings);
    const offered = catalogForProvider({ providers }, offeredHarness(providers, choice.provider));
    const models = offered.models || [];
    const takesEffort = effortSupported(models, choice.model);
    harness.innerHTML = providerOptionsHtml(providers, choice.provider);
    model.innerHTML = modelOptionsHtml(models, choice.model);
    effort.innerHTML = effortOptionsHtml(
      offered.efforts || [],
      takesEffort ? choice.effort : "",
      modelInCatalog(models, choice.model),
    );
    for (const control of controls) control.disabled = false;
    effort.disabled = !takesEffort;
  };

  /** One change, written and confirmed. A refusal says so and puts the controls
   *  back on what the device holds, so nothing on screen is a choice the
   *  machine never took. */
  const save = async (patch) => {
    for (const control of controls) control.disabled = true;
    error.textContent = "";
    saved.textContent = "Saving…";
    try {
      const settings = await callRpc("settings.set", { project_agent: patch });
      renderConfirmedSettings(settings);
      await onSaved(settings);
      saved.textContent = "Saved. New project agents start here.";
    } catch (saveError) {
      error.textContent = saveError.message;
      saved.textContent = "";
      try {
        renderConfirmedSettings(await callRpc("settings.get"));
      } catch (reloadError) {
        clearUnconfirmedSelection();
        error.textContent = `${saveError.message}. Reload failed: ${reloadError.message}`;
      }
    }
  };

  // A model belongs to its harness and an effort to its model, so moving the
  // one above drops what was chosen under it. An emptied select is "no
  // preference", which on the wire is the `null` that clears the word.
  harness.onchange = () => save({ provider: harness.value, model: null, effort: null });
  model.onchange = () => save({ model: model.value || null, effort: null });
  effort.onchange = () => save({ effort: effort.value || null });

  try {
    const [settings, catalog] = await Promise.all([callRpc("settings.get"), callRpc("models.list")]);
    if (!Array.isArray(catalog?.providers) || catalog.providers.length === 0) {
      throw new Error("models.list.providers is missing, malformed, or empty");
    }
    providers = catalog.providers;
    renderConfirmedSettings(settings);
  } catch (readError) {
    clearUnconfirmedSelection();
    error.textContent = readError.message;
  }
}
