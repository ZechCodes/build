// Provider/model/effort selector logic. The catalog comes from the bridge's
// models.list RPC, so new harness choices arrive with bridge updates. An empty
// model means the selected agent CLI's own configured default.

import { esc } from "./text.js";
import {
  catalogForProvider,
  creatableCatalog,
  normalizeModelCatalog,
  providerLabel,
  STARTABLE_PROVIDERS,
} from "./providerCatalog.js";

export {
  catalogForProvider,
  creatableCatalog,
  normalizeModelCatalog,
  providerLabel,
  STARTABLE_PROVIDERS,
};

export function providerOptionsHtml(providers, selectedId) {
  return providers
    .map((provider) => `<option value="${esc(provider.id)}"${provider.id === selectedId ? " selected" : ""}>${esc(provider.label)}</option>`)
    .join("");
}

/** Pure: the harnesses as a card picker. Used where the choice is made BEFORE
 *  the thing it configures exists — the chat tab of a work item with no agent —
 *  so there is nothing yet to hang a `<select>` off. Each card is a real button
 *  carrying its id, so a press reads the answer straight off
 *  `dataset.provider`; `selectedId` marks the chosen one. */
export function providerCardsHtml(providers, selectedId) {
  const cards = (providers || [])
    .map(
      (provider) =>
        `<button class="chooser-card${provider.id === selectedId ? " chosen" : ""}" type="button" data-provider="${esc(provider.id)}">
          <span class="chooser-card-label">${esc(provider.label)}</span>
        </button>`,
    )
    .join("");
  return `<div class="chooser">
    <div class="chooser-head">Which agent works here?</div>
    <div class="chooser-cards">${cards}</div>
  </div>`;
}

export function modelInCatalog(models, modelId) {
  return (models || []).find((model) => model.id === modelId) || null;
}

export function matchCatalogModel(models, modelId) {
  if (!modelId) return null;
  const exact = modelInCatalog(models, modelId);
  if (exact) return exact;
  const undated = modelId.replace(/-\d{6,8}$/, "");
  return undated === modelId ? null : modelInCatalog(models, undated);
}

/** <option> list for the model select: harness default, catalog, and — when the
 *  current selection is not in the catalog (e.g. a task dispatched on a newer
 *  bridge, or a model this machine's CLI is too old for) — the selection
 *  itself, so it never silently changes. `provider` is the catalog entry the
 *  models came from, which names what its CLI is too old for. */
export function modelOptionsHtml(models, selectedId, provider = null) {
  const sel = (id) => (id === (selectedId || "") ? " selected" : "");
  const rows = [`<option value=""${sel("")}>Harness default</option>`];
  for (const m of models) {
    rows.push(`<option value="${esc(m.id)}"${sel(m.id)}>${esc(m.label)}</option>`);
  }
  if (selectedId && !models.some((m) => m.id === selectedId)) {
    rows.push(`<option value="${esc(selectedId)}" selected>${esc(offCatalogLabel(selectedId, provider))}</option>`);
  }
  return rows.join("");
}

/** What a selection the picker does not offer is called: the model and the
 *  CLI version it needs, where the bridge said; else its id. */
function offCatalogLabel(selectedId, provider) {
  const needed = unavailableOf(provider).find((model) => model.id === selectedId);
  if (!needed) return selectedId;
  return `${needed.label} (needs ${provider.cli_name} ${needed.requires_cli}+)`;
}

function unavailableOf(provider) {
  return Array.isArray(provider?.unavailable) ? provider.unavailable : [];
}

/** The one line under a model picker when this machine's CLI is too old for
 *  some of the harness's models: the CLI, the version that brings all of
 *  them, and which they are. Empty when it runs everything, or the bridge
 *  predates saying so (#203). */
export function modelUpdateNote(provider) {
  const missing = unavailableOf(provider);
  if (!missing.length) return "";
  const version = missing.map((model) => model.requires_cli).reduce(newerVersion);
  return `Update ${provider.cli_name} to ${version}+ for ${sentenceList(missing.map((model) => model.label))}.`;
}

export function modelNoteHtml(provider) {
  const note = modelUpdateNote(provider);
  return note ? `<div class="model-update-note">${esc(note)}</div>` : "";
}

function newerVersion(a, b) {
  const parts = (version) => String(version).split(".").map((part) => parseInt(part, 10) || 0);
  const [left, right] = [parts(a), parts(b)];
  for (let at = 0; at < Math.max(left.length, right.length); at += 1) {
    const difference = (left[at] || 0) - (right[at] || 0);
    if (difference) return difference > 0 ? a : b;
  }
  return a;
}

function sentenceList(words) {
  if (words.length < 2) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

/** Whether the effort select applies to the chosen model. Unknown ids pass
 *  through (the bridge validates); only catalog models that declare no effort
 *  support disable it. */
export function effortSupported(models, selectedId) {
  if (!selectedId) return true;
  const entry = models.find((m) => m.id === selectedId);
  return entry ? entry.supports_effort : true;
}

/** The reasoning levels on offer: the chosen model's own where it names them,
 *  else the harness's. */
export function effortLevels(efforts, model) {
  return (model && Array.isArray(model.efforts) ? model.efforts : efforts) || [];
}

export function effortOptionsHtml(efforts, selected, model = null) {
  const sel = (v) => (v === (selected || "") ? " selected" : "");
  return [
    `<option value=""${sel("")}>Default effort</option>`,
    ...effortLevels(efforts, model).map((e) => `<option value="${esc(e)}"${sel(e)}>${esc(e)}</option>`),
  ].join("");
}

/** The dispatch/approve params fragment for a selection: empties are omitted
 *  (harness default), and effort is dropped for models that don't support it. */
export function modelParams(models, model, effort, provider) {
  const params = {};
  if (provider) params.provider = provider;
  if (model) params.model = model;
  if (effort && effortSupported(models, model)) params.effort = effort;
  return params;
}
