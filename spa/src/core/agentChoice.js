// The provider / model / effort choice, as one panel.
//
// Two places ask the same question — the compose box's advanced panel and the
// toolbar's create menu — and both mean exactly the same thing by it, so they
// ask it with the same component. The catalog is one bridge's (models.list,
// held on that device's context). An empty model or effort means the harness's
// own default; the provider always resolves against the catalog the panel
// displays.

import { esc } from "./text.js";
import {
  catalogForProvider,
  creatableCatalog,
  effortLevels,
  effortOptionsHtml,
  effortSupported,
  matchCatalogModel,
  modelInCatalog,
  modelNoteHtml,
  modelUpdateNote,
  modelOptionsHtml,
  modelParams,
  providerOptionsHtml,
} from "./modelPicker.js";
import { providerInSameFamily } from "./providerCatalog.js";

/** No stored preferences; the provider resolves against the displayed offer. */
export const NO_AGENT_CHOICE = { provider: "", model: "", effort: "" };

const offeredProviderId = (providers, providerId) =>
  providers.find((provider) => provider.id === providerId)?.id
  || providerInSameFamily(providers, providerId);

/** The provider a choice is really on: the one it named, else the catalog's
 *  default, else the first the daemon offers.
 *
 *  Over a narrowed catalog this is also what clamps a stale preference: a token
 *  the offer does not hold is answered with the default, so a provider stored
 *  when a third harness was on offer paints and dispatches as one of the two. */
export function chosenProviderId(catalog, choice) {
  const providers = (catalog && catalog.providers) || [];
  const named = choice && choice.provider;
  const chosen = offeredProviderId(providers, named);
  if (chosen) return chosen;
  const fallback = catalog && catalog.default_provider;
  const familyFallback = offeredProviderId(providers, fallback);
  if (familyFallback) return familyFallback;
  return providers[0] ? providers[0].id : "";
}

/**
 * The three selects, behind a disclosure of their own. Rendering them shut is
 * the point: picking a harness is the rare act, and the common one is not
 * having to.
 *
 * `prefix` names the three controls, so two panels can be open at once without
 * either one answering for the other.
 *
 * Both callers create or dispatch, so the Agent select offers the two agents a
 * person can create — never the carrier question behind Claude Code, which the
 * account has already answered. An agent that already exists is locked to its
 * own harness and asks none of this: its picker is the composer's model menu.
 */
// eslint-disable-next-line complexity -- ratchet: agentChoicePanelHtml is at 11, cap 10 — reduce it, then drop this line
export function agentChoicePanelHtml(catalog, choice, { prefix = "agent-choice", open = false } = {}) {
  const offered = creatableCatalog(catalog || {});
  const providerId = chosenProviderId(offered, choice);
  const forProvider = catalogForProvider(offered, providerId);
  const models = forProvider.models || [];
  const model = modelInCatalog(models, choice.model);
  return `<div class="agent-choice">
    <button class="compose-disclose" type="button" data-agent-choice-toggle="${esc(prefix)}" aria-expanded="${open ? "true" : "false"}">
      <span class="disclosure-caret" aria-hidden="true">${open ? "▾" : "▸"}</span> Agent, model and effort</button>
    <div class="agent-choice-fields"${open ? "" : " hidden"}>
      <label for="${esc(prefix)}-provider">Agent</label>
      <select id="${esc(prefix)}-provider">${providerOptionsHtml(offered.providers, providerId)}</select>
      <label for="${esc(prefix)}-model">Model</label>
      <select id="${esc(prefix)}-model">${modelOptionsHtml(models, choice.model, forProvider)}</select>
      ${modelNoteHtml(forProvider)}
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

/** The choice as create/dispatch params: empty model and effort are omitted (the
 *  harness's own defaults stand), and unsupported effort is dropped.
 *
 *  The provider sent is the CLAMPED one — what the select painted — so a stale
 *  token or an unavailable account default cannot ride out on the wire under a
 *  control that showed another name. */
export function agentChoiceParams(catalog, choice) {
  const offered = creatableCatalog(catalog || {});
  const providerId = chosenProviderId(offered, choice);
  const models = catalogForProvider(offered, providerId).models || [];
  return modelParams(models, choice.model, choice.effort, providerId);
}

// ---- the composer's model menu ---------------------------------------------
//
// The same question the panel above asks, minus the harness: an agent is locked
// to the one it was created on, so the composer's menu asks only what is still
// open. The rows are the split button's menu half (core/splitButton.js), and
// each one's id carries the field it sets, so a press says both what changed
// and what to.

const MENU_FIELD_SEPARATOR = ":";
const menuOption = (field, value, label, description, selected) => ({
  id: `${field}${MENU_FIELD_SEPARATOR}${value}`,
  label,
  description,
  selected,
});

/** The menu's rows for one agent: its harness's models, then the reasoning
 *  levels the chosen model takes — a model that takes none is not asked. */
export function modelMenuOptions(catalog, providerId, choice) {
  const forProvider = catalogForProvider(catalog || {}, providerId);
  const models = forProvider.models || [];
  const rows = [
    menuOption("model", "", "Harness default", "the model the agent's own config picks", !choice.model),
    ...models.map((model) =>
      menuOption("model", model.id, model.label, "", model.id === choice.model),
    ),
  ];
  if (!effortSupported(models, choice.model)) return rows;
  const levels = effortLevels(forProvider.efforts, modelInCatalog(models, choice.model));
  return [
    ...rows,
    menuOption("effort", "", "Default effort", "", !choice.effort),
    ...levels.map((level) => menuOption("effort", level, level, "", level === choice.effort)),
  ];
}

/** The line under the menu's models when this machine's CLI is too old for
 *  some of them (#203): the menu lists only what it can run, so this is where
 *  the rest are named. */
export function modelMenuNote(catalog, providerId) {
  return modelUpdateNote(catalogForProvider(catalog || {}, providerId));
}

/** The model and reasoning controls are separate in the composer so both
 * choices remain visible without opening an ambiguously named combined menu. */
export function modelSelectorOptions(catalog, providerId, choice) {
  return modelMenuOptions(catalog, providerId, choice)
    .filter((option) => option.id.startsWith("model:") && option.id !== "model:");
}

const effectiveEffort = (catalog, providerId, choice, activeModel, activeEffort) =>
  choice.effort || (!movesAtNextStart(catalog, providerId, choice, activeModel) && activeEffort) || "";

export function reasoningSelectorOptions(catalog, providerId, choice, activeModel = "", activeEffort = "") {
  const selectedEffort = effectiveEffort(catalog, providerId, choice, activeModel, activeEffort);
  const models = catalogForProvider(catalog || {}, providerId).models || [];
  const effectiveModel = choice.model || activeModel;
  const model = matchCatalogModel(models, effectiveModel)?.id || effectiveModel;
  return modelMenuOptions(catalog, providerId, { ...choice, model })
    .filter((option) => option.id.startsWith("effort:") && option.id !== "effort:")
    .map((option) => ({ ...option, selected: option.id === `effort:${selectedEffort}` }));
}

export function reasoningSelectorLabel(catalog, providerId, choice, activeModel = "", activeEffort = "") {
  return effectiveEffort(catalog, providerId, choice, activeModel, activeEffort) || "Default effort";
}

export function activeModelLabel(catalog, providerId, modelId) {
  if (!modelId) return "";
  const model = matchCatalogModel(catalogForProvider(catalog || {}, providerId).models || [], modelId);
  return model ? model.label : modelId;
}

const capitalized = (word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
const versionOf = (major, minor) => (minor ? `${major}.${minor}` : major);

/** How a compact row names a model (#257): a Claude family and its version,
 *  or a GPT version and its codename. The version is one or two digits, so a
 *  date after it ("-20251001") is never read as one. */
const SHORT_MODEL_RULES = [
  {
    pattern: /\b(opus|sonnet|haiku|fable)[-\s]+(\d{1,2})(?:[-.\s](\d{1,2}))?(?!\d)/i,
    name: ([, family, major, minor]) => `${capitalized(family)} ${versionOf(major, minor)}`,
  },
  {
    pattern: /\bgpt[-\s]?(\d{1,2})(?:[-.](\d{1,2}))?[-\s]+([a-z]+(?:[-\s][a-z]+)*)\b/i,
    name: ([, major, minor, codename]) => `${versionOf(major, minor)} ${codename.split(/[-\s]/).map(capitalized).join(" ")}`,
  },
];

/** A model id or label as its short name ("claude-opus-5-5" → "Opus 5.5",
 *  "gpt-6-astra" → "6 Astra"), or "" for one no rule knows. */
export function shortModelName(text) {
  const source = String(text || "");
  for (const rule of SHORT_MODEL_RULES) {
    const match = source.match(rule.pattern);
    if (match) return rule.name(match);
  }
  return "";
}

/** A model's short name, read from its id and else from its full label,
 *  falling back to that full label. */
export const shortModelNameOr = (modelId, fullLabel) =>
  shortModelName(modelId) || shortModelName(fullLabel) || fullLabel;

/** The model as a compact row wears it (#257). Blank only when there is no
 *  model. Full names stay in tooltips and the picker. */
export function shortModelLabel(catalog, providerId, modelId) {
  return shortModelNameOr(modelId, activeModelLabel(catalog, providerId, modelId));
}

const withChoiceEffort =(name, choice) => (choice.effort ? `${name} · ${choice.effort}` : name);

const pendingModelLabel = (catalog, providerId, choice) => {
  const model = modelInCatalog(catalogForProvider(catalog || {}, providerId).models || [], choice.model);
  return withChoiceEffort(model ? model.label : choice.model, choice);
};

const runsThePendingModel = (catalog, providerId, choice, activeModel) => {
  const models = catalogForProvider(catalog || {}, providerId).models || [];
  const active = matchCatalogModel(models, activeModel);
  const pending = modelInCatalog(models, choice.model);
  if (active && pending) return active.id === pending.id;
  return activeModel === choice.model;
};

export function movesAtNextStart(catalog, providerId, choice, activeModel) {
  if (!activeModel || !choice.model) return false;
  return !runsThePendingModel(catalog, providerId, choice, activeModel);
}

export function modelMenuLabel(catalog, providerId, choice, activeModel = "") {
  if (!activeModel) return choice.model ? pendingModelLabel(catalog, providerId, choice) : "Harness default";
  const active = activeModelLabel(catalog, providerId, activeModel);
  if (!movesAtNextStart(catalog, providerId, choice, activeModel)) return withChoiceEffort(active, choice);
  return `${active} → ${pendingModelLabel(catalog, providerId, choice)}`;
}

export function modelMenuTitle(catalog, providerId, choice, activeModel = "") {
  if (!activeModel) return "Model and reasoning effort";
  const running = `Running ${activeModelLabel(catalog, providerId, activeModel)}.`;
  if (movesAtNextStart(catalog, providerId, choice, activeModel)) {
    return `${running} ${pendingModelLabel(catalog, providerId, choice)} at the next start.`;
  }
  if (!choice.model) return `${running} Select the model for the next start.`;
  return "Model and reasoning effort";
}

/** The choice one press makes, reconciled: a model change drops the effort that
 *  hung off the model before it. The harness is never touched. */
export function modelMenuSelection(actionId, choice) {
  const separator = String(actionId || "").indexOf(MENU_FIELD_SEPARATOR);
  const current = { ...NO_AGENT_CHOICE, ...choice };
  if (separator === -1) return current;
  const field = actionId.slice(0, separator);
  const value = actionId.slice(separator + 1);
  if (field === "model") return reconcileAgentChoice({ ...current, model: value }, { modelChanged: true });
  if (field === "effort") return { ...current, effort: value };
  return current;
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
