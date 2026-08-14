// The provider / model / effort choice, as one panel.
//
// Two places ask the same question — the compose box's advanced panel and the
// toolbar's create menu — and both mean exactly the same thing by it, so they
// ask it with the same component. The catalog is the bridge's (models.list, via
// App.modelCatalog); an empty choice means the harness's own default, which is
// what "no preference" has to be able to say.

import { esc } from "./text.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelOptionsHtml,
  modelParams,
  providerOptionsHtml,
} from "./modelPicker.js";

/** The empty choice: whatever the daemon's catalog says is default. */
export const NO_AGENT_CHOICE = { provider: "", model: "", effort: "" };

const providersOf = (catalog) => (catalog && catalog.providers) || [];

/** The provider a choice is really on: the one it named, else the catalog's
 *  default, else the first the daemon offers. */
export function chosenProviderId(catalog, choice) {
  const providers = providersOf(catalog);
  const named = choice && choice.provider;
  if (named && providers.some((provider) => provider.id === named)) return named;
  const fallback = catalog && catalog.default_provider;
  if (fallback && providers.some((provider) => provider.id === fallback)) return fallback;
  return providers[0] ? providers[0].id : "";
}

/**
 * The three selects, behind a disclosure of their own. Rendering them shut is
 * the point: picking a harness is the rare act, and the common one is not
 * having to.
 *
 * `prefix` names the three controls, so two panels can be open at once without
 * either one answering for the other.
 */
export function agentChoicePanelHtml(catalog, choice, { prefix = "agent-choice", open = false } = {}) {
  const providerId = chosenProviderId(catalog, choice);
  const forProvider = catalogForProvider(catalog || {}, providerId);
  const models = forProvider.models || [];
  const model = modelInCatalog(models, choice.model);
  return `<div class="agent-choice">
    <button class="compose-disclose" type="button" data-agent-choice-toggle="${esc(prefix)}" aria-expanded="${open ? "true" : "false"}">
      ${open ? "▾" : "▸"} Agent, model and effort</button>
    <div class="agent-choice-fields"${open ? "" : " hidden"}>
      <label for="${esc(prefix)}-provider">Agent</label>
      <select id="${esc(prefix)}-provider">${providerOptionsHtml(
        providersOf(catalog).map((provider) => ({ id: provider.id, label: provider.label || provider.id })),
        providerId,
      )}</select>
      <label for="${esc(prefix)}-model">Model</label>
      <select id="${esc(prefix)}-model">${modelOptionsHtml(models, choice.model)}</select>
      <label for="${esc(prefix)}-effort">Effort</label>
      <select id="${esc(prefix)}-effort"${effortSupported(models, choice.model) ? "" : " disabled"}>${effortOptionsHtml(
        forProvider.efforts || [],
        choice.effort,
        model,
      )}</select>
    </div>
  </div>`;
}

/** What the panel's three controls currently say. */
export function readAgentChoice(root, prefix = "agent-choice") {
  const valueOf = (field) => {
    const control = root.querySelector(`#${prefix}-${field}`);
    return control ? control.value : "";
  };
  return { provider: valueOf("provider"), model: valueOf("model"), effort: valueOf("effort") };
}

/** The choice as create/dispatch params: empties are omitted (the harness's own
 *  default stands), and an effort the model does not support is dropped. */
export function agentChoiceParams(catalog, choice) {
  const models = catalogForProvider(catalog || {}, chosenProviderId(catalog, choice)).models || [];
  return modelParams(models, choice.model, choice.effort, choice.provider);
}

/** A model belongs to its provider and an effort to its model, so changing one
 *  drops what hung off it. Pure, so every panel agrees. */
export function reconcileAgentChoice(choice, { providerChanged = false, modelChanged = false } = {}) {
  const next = { ...NO_AGENT_CHOICE, ...choice };
  if (providerChanged) {
    next.model = "";
    next.effort = "";
  } else if (modelChanged) {
    next.effort = "";
  }
  return next;
}
