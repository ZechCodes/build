// Provider/model/effort selector logic. The catalog comes from the bridge's
// models.list RPC, so new harness choices arrive with bridge updates. An empty
// model means the selected agent CLI's own configured default.

import { esc } from "./text.js";

/** The agents a start can name. The catalog RPC is the authority for models and
 *  efforts, but a start needs only a provider — so the Agent tab's picker
 *  renders with the pane, not after a round trip.
 *
 *  One card per agent a person knows, not one per carrier: Claude Code runs two
 *  carriers, and which one a start opens is the account's answer (Account →
 *  Settings), resolved by the bridge. A start names "claude" and gets whichever
 *  program the account says that is. */
export const STARTABLE_PROVIDERS = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];

/** What a start leads with where nothing has run yet to say otherwise. */
export const DEFAULT_START_PROVIDER = "claude";

/** Every provider id the bridge can name, and what a person calls it. Both
 *  claude carriers are "Claude Code": a record persisted on either one is the
 *  same agent to the human, and the account setting is what keeps two of them
 *  from ever sitting side by side. */
const PROVIDER_LABELS = {
  claude: "Claude Code",
  claude_adk: "Claude Code",
  codex: "Codex",
};

/** The provider's name as a person says it. An unknown provider is shown as the
 *  bridge named it — a new harness must read as itself, not as "Agent". */
export function providerLabel(provider) {
  if (!provider) return "Agent";
  return PROVIDER_LABELS[provider] || String(provider);
}

export function providerOptionsHtml(providers, selectedId) {
  return providers
    .map((provider) => `<option value="${esc(provider.id)}"${provider.id === selectedId ? " selected" : ""}>${esc(provider.label)}</option>`)
    .join("");
}

/** Pure: one provider as a picker card — a real button carrying its id, so a
 *  click reads the answer straight off `dataset.provider`. */
export function providerCardHtml(provider, { selected = false, className = "", id = "" } = {}) {
  const classes = `chooser-card${selected ? " chosen" : ""}${className ? ` ${className}` : ""}`;
  return `<button class="${classes}" type="button"${id ? ` id="${esc(id)}"` : ""} data-provider="${esc(provider.id)}">
          <span class="chooser-card-label">${esc(provider.label)}</span>${
            provider.description ? `<span class="chooser-card-desc">${esc(provider.description)}</span>` : ""
          }
        </button>`;
}

/** Pure: the providers as a card picker. Used where the choice is made BEFORE
 *  the thing it configures exists (the new-worktree sheet), so there is nothing
 *  yet to hang a `<select>` off. `selectedId` marks the chosen card.
 *
 *  `options.lead` is one provider promoted above the row as a full-width card
 *  (`{ id, label, description }`, given `options.leadId` as its element id):
 *  the Agent tab leads with the harness that worktree already ran, without
 *  taking the other one away. */
export function providerCardsHtml(providers, selectedId, { lead = null, leadId = "" } = {}) {
  const cards = (providers || [])
    .map((provider) => providerCardHtml(provider, { selected: provider.id === selectedId }))
    .join("");
  return `<div class="chooser">
    <div class="chooser-head">Which agent works here?</div>
    ${lead ? providerCardHtml(lead, { className: "chooser-card-lead", id: leadId }) : ""}
    <div class="chooser-cards">${cards}</div>
  </div>`;
}

export function catalogForProvider(catalog, providerId) {
  const providers = catalog.providers || [];
  return providers.find((provider) => provider.id === providerId) || providers[0] || { models: [], efforts: [] };
}

/** The name a catalog entry is offered under. */
const catalogName = (provider) => provider.label || providerLabel(provider.id);

/** The catalog carries one entry per carrier — a run persisted on either claude
 *  carrier needs its models served under its own id — but two entries under one
 *  name is a picker asking a question with the same answer twice. Keep the first
 *  of each name; whichever carrier the bridge lists first is the one a fresh
 *  choice is made on, and the account setting decides what it opens. */
function oneProviderPerName(providers) {
  const kept = [];
  for (const provider of providers) {
    if (!kept.some((entry) => catalogName(entry) === catalogName(provider))) kept.push(provider);
  }
  return kept;
}

/** The default, moved onto the entry that survived the fold when the bridge's
 *  default was the carrier that did not. */
function defaultAmong(providers, defaultProvider) {
  if (!defaultProvider || providers.some((provider) => provider.id === defaultProvider)) return defaultProvider;
  const folded = providers.find((provider) => providerLabel(provider.id) === providerLabel(defaultProvider));
  return folded ? folded.id : defaultProvider;
}

export function normalizeModelCatalog(catalog) {
  if (catalog && Array.isArray(catalog.providers) && catalog.providers.length) {
    const providers = oneProviderPerName(catalog.providers);
    return { ...catalog, providers, default_provider: defaultAmong(providers, catalog.default_provider) };
  }
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
