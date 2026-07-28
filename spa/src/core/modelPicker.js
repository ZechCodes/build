// Provider/model/effort selector logic. The catalog comes from the bridge's
// models.list RPC, so new harness choices arrive with bridge updates. An empty
// model means the selected agent CLI's own configured default.

import { esc } from "./text.js";

export function providerOptionsHtml(providers, selectedId) {
  return providers
    .map((provider) => `<option value="${esc(provider.id)}"${provider.id === selectedId ? " selected" : ""}>${esc(provider.label)}</option>`)
    .join("");
}

/** Pure: the providers as a card picker. Used where the choice is made BEFORE
 *  the thing it configures exists (the new-worktree sheet), so there is nothing
 *  yet to hang a `<select>` off. `selectedId` marks the chosen card. */
export function providerCardsHtml(providers, selectedId) {
  const cards = (providers || [])
    .map(
      (provider) =>
        `<button class="chooser-card${provider.id === selectedId ? " chosen" : ""}" type="button" data-provider="${esc(provider.id)}">
          <span class="chooser-card-label">${esc(provider.label)}</span>${
            provider.description ? `<span class="chooser-card-desc">${esc(provider.description)}</span>` : ""
          }
        </button>`,
    )
    .join("");
  return `<div class="chooser">
    <div class="chooser-head">Which agent works here?</div>
    <div class="chooser-cards">${cards}</div>
  </div>`;
}

export function catalogForProvider(catalog, providerId) {
  const providers = catalog.providers || [];
  return providers.find((provider) => provider.id === providerId) || providers[0] || { models: [], efforts: [] };
}

export function normalizeModelCatalog(catalog) {
  if (catalog && Array.isArray(catalog.providers) && catalog.providers.length) return catalog;
  return {
    default_provider: "claude",
    providers: [{
      id: "claude",
      label: "Claude Code",
      models: (catalog && catalog.models) || [],
      efforts: (catalog && catalog.efforts) || [],
    }],
  };
}

export function modelInCatalog(models, modelId) {
  return (models || []).find((model) => model.id === modelId) || null;
}

/** <option> list for the model select: harness default, catalog, and — when the
 *  current selection is not in the catalog (e.g. a task dispatched on a newer
 *  bridge) — the selection itself, so it never silently changes. */
export function modelOptionsHtml(models, selectedId) {
  const sel = (id) => (id === (selectedId || "") ? " selected" : "");
  const rows = [`<option value=""${sel("")}>Harness default</option>`];
  for (const m of models) {
    rows.push(`<option value="${esc(m.id)}"${sel(m.id)}>${esc(m.label)}</option>`);
  }
  if (selectedId && !models.some((m) => m.id === selectedId)) {
    rows.push(`<option value="${esc(selectedId)}" selected>${esc(selectedId)}</option>`);
  }
  return rows.join("");
}

/** Whether the effort select applies to the chosen model. Unknown ids pass
 *  through (the bridge validates); only catalog models that declare no effort
 *  support disable it. */
export function effortSupported(models, selectedId) {
  if (!selectedId) return true;
  const entry = models.find((m) => m.id === selectedId);
  return entry ? entry.supports_effort : true;
}

export function effortOptionsHtml(efforts, selected, model = null) {
  const available = model && Array.isArray(model.efforts) ? model.efforts : efforts;
  const sel = (v) => (v === (selected || "") ? " selected" : "");
  return [
    `<option value=""${sel("")}>Default effort</option>`,
    ...available.map((e) => `<option value="${esc(e)}"${sel(e)}>${esc(e)}</option>`),
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
