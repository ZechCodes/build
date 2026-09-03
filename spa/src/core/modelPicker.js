// Provider/model/effort selector logic. The catalog comes from the bridge's
// models.list RPC, so new harness choices arrive with bridge updates. An empty
// model means the selected agent CLI's own configured default.

import { esc } from "./text.js";

/** Every harness there is, and what a person calls each one.
 *
 *  One entry per harness, because an agent is LOCKED to the one it was created
 *  on: its conversation lives in that program, so it never moves, and two
 *  agents on two carriers sit side by side and must not read alike.
 *
 *  This is the naming vocabulary, not an offer. What a create surface offers is
 *  two agents (`creatableCatalog`); carrier-specific names are spoken where a
 *  harness is being NAMED — an agent's own bubble, and the Account setting that
 *  decides which carrier a new agent gets. */
export const STARTABLE_PROVIDERS = [
  { id: "claude_adk", label: "Claude Code" },
  { id: "claude", label: "Claude Code TUI" },
  { id: "codex_app_server", label: "Codex" },
  { id: "codex", label: "Codex TUI" },
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

const CARRIERS_BY_GENERIC_PROVIDER = {
  claude_adk: ["claude_adk", "claude"],
  codex_app_server: ["codex_app_server", "codex"],
};

export function concreteProviderId(genericProviderId, defaultProviderId) {
  const carriers = CARRIERS_BY_GENERIC_PROVIDER[genericProviderId];
  if (!carriers) throw new Error(`Unknown generic provider: ${genericProviderId}`);
  return carriers.includes(defaultProviderId) ? defaultProviderId : genericProviderId;
}

/** The two agents a person can create, with the account's answer folded in: the
 *  generic card carries its TUI carrier only when the account's own default is
 *  that TUI carrier.
 *
 *  Which carrier opens is an account question, answered once in Settings — so
 *  a create surface asks which agent, never which carrier. */
export function creatableAgents(defaultProviderId) {
  return Object.keys(CARRIERS_BY_GENERIC_PROVIDER).map((genericProviderId) => ({
    id: concreteProviderId(genericProviderId, defaultProviderId),
    label: providerLabel(genericProviderId),
  }));
}

/** The catalog a create surface offers: exactly the two agents, each carrying
 *  the models the bridge listed for the carrier behind it. Two entries even
 *  before models.list answers — creating an agent needs only a harness, and the
 *  cards paint before the round trip.
 *
 *  Built from `creatableAgents` rather than filtered out of the catalog, so the
 *  labels are the client's own vocabulary: an older bridge that calls both
 *  claude carriers the same thing still cannot print one name twice. */
export function creatableCatalog(catalog) {
  const served = (catalog && catalog.providers) || [];
  const providers = creatableAgents(catalog && catalog.default_provider).map((agent) => {
    const listed = served.find((provider) => provider.id === agent.id) || {};
    return { ...agent, models: listed.models || [], efforts: listed.efforts || [] };
  });
  return { ...catalog, providers };
}

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

export function matchCatalogModel(models, modelId) {
  if (!modelId) return null;
  const exact = modelInCatalog(models, modelId);
  if (exact) return exact;
  const undated = modelId.replace(/-\d{6,8}$/, "");
  return undated === modelId ? null : modelInCatalog(models, undated);
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
