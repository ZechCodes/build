// Provider/model/effort selector logic. The catalog comes from the bridge's
// models.list RPC, so new harness choices arrive with bridge updates. An empty
// model means the selected agent CLI's own configured default.

import { esc } from "./text.js";

/** The harnesses an agent can be created on. The catalog RPC is the authority
 *  for models and efforts, but creating an agent needs only a harness — so a
 *  picker renders with its surface, not after a round trip.
 *
 *  One entry per harness, because an agent is LOCKED to the one it was created
 *  on: its conversation lives in that program, so it never moves. Which harness
 *  a NEW agent is created on where nobody said is the account's answer
 *  (`models.list`'s `default_provider`). */
export const STARTABLE_PROVIDERS = [
  { id: "claude_adk", label: "Claude Code" },
  { id: "claude", label: "Claude Code TUI" },
  { id: "codex", label: "Codex" },
];

/** Every harness the bridge can name, and what a person calls it. */
const PROVIDER_LABELS = Object.fromEntries(
  STARTABLE_PROVIDERS.map((provider) => [provider.id, provider.label]),
);

/** The harness's name as a person says it. An unknown provider is shown as the
 *  bridge named it — a new harness must read as itself, not as "Agent". */
export function providerLabel(provider) {
  if (!provider) return "Agent";
  return PROVIDER_LABELS[provider] || String(provider);
}

/** The catalog entries a picker may offer: the harnesses an agent can be
 *  created on, in the client's own vocabulary — an older bridge that called
 *  both claude harnesses the same thing must not offer one name twice. */
export function startableCatalogProviders(providers) {
  const startable = STARTABLE_PROVIDERS.map((provider) => provider.id);
  return (providers || [])
    .filter((provider) => startable.includes(provider.id))
    .map((provider) => ({ ...provider, label: providerLabel(provider.id) }));
}

export function providerOptionsHtml(providers, selectedId) {
  return providers
    .map((provider) => `<option value="${esc(provider.id)}"${provider.id === selectedId ? " selected" : ""}>${esc(provider.label)}</option>`)
    .join("");
}

/** Pure: one provider as a picker card — a real button carrying its id, so a
 *  click reads the answer straight off `dataset.provider`. */
export function providerCardHtml(provider, { selected = false } = {}) {
  return `<button class="chooser-card${selected ? " chosen" : ""}" type="button" data-provider="${esc(provider.id)}">
          <span class="chooser-card-label">${esc(provider.label)}</span>${
            provider.description ? `<span class="chooser-card-desc">${esc(provider.description)}</span>` : ""
          }
        </button>`;
}

/** Pure: the providers as a card picker. Used where the choice is made BEFORE
 *  the thing it configures exists — the new-worktree sheet, and the chat tab of
 *  a work item with no agent — so there is nothing yet to hang a `<select>`
 *  off. `selectedId` marks the chosen card. */
export function providerCardsHtml(providers, selectedId) {
  const cards = (providers || [])
    .map((provider) => providerCardHtml(provider, { selected: provider.id === selectedId }))
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

/** The catalog every picker reads. A bridge that lists its harnesses is taken
 *  as it stands — each one is an agent a person can create, and an agent locked
 *  to one of them needs its own models under its own id. A bridge too old to
 *  list any answers with one flat catalog, which stands up as the default
 *  harness's. */
export function normalizeModelCatalog(catalog) {
  if (catalog && Array.isArray(catalog.providers) && catalog.providers.length) return { ...catalog };
  const id = (catalog && catalog.default_provider) || STARTABLE_PROVIDERS[0].id;
  return {
    default_provider: id,
    providers: [{
      id,
      label: providerLabel(id),
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
