const PROVIDER_FAMILIES = [
  {
    key: "claude",
    genericLabel: "Claude Code",
    fallbackCarrierId: "claude_adk",
    modeCarrierIds: { headless: "claude_adk", tui: "claude" },
    carriers: [
      { id: "claude_adk", label: "Claude Code" },
      { id: "claude", label: "Claude Code TUI" },
    ],
  },
  {
    key: "codex",
    genericLabel: "Codex",
    fallbackCarrierId: "codex",
    modeCarrierIds: { headless: "codex_app_server", tui: "codex" },
    carriers: [
      { id: "codex_app_server", label: "Codex", requiresProviderCatalog: true },
      { id: "codex", label: "Codex TUI" },
    ],
  },
];

export const STARTABLE_PROVIDERS = PROVIDER_FAMILIES.flatMap((family) =>
  family.carriers.map(({ id, label }) => ({ id, label })),
);

const PROVIDER_LABELS = Object.fromEntries(
  STARTABLE_PROVIDERS.map((provider) => [provider.id, provider.label]),
);

export function providerLabel(providerId) {
  if (!providerId) return "Agent";
  return PROVIDER_LABELS[providerId] || String(providerId);
}

function providerFamilyForCarrier(providerId) {
  return PROVIDER_FAMILIES.find((family) =>
    family.carriers.some((carrier) => carrier.id === providerId),
  );
}

export function providerInSameFamily(providers, providerId) {
  const family = providerFamilyForCarrier(providerId);
  return family?.carriers.find((carrier) =>
    providers.some((provider) => provider.id === carrier.id),
  )?.id || "";
}

function carrierForMode(family, mode) {
  const id = family.modeCarrierIds[mode];
  return family.carriers.find((carrier) => carrier.id === id);
}

function explicitCarrierId(family, catalog) {
  const carrier = carrierForMode(family, catalog?.agent_modes?.[family.key]);
  if (!carrier) return "";
  if (!carrier.requiresProviderCatalog) return carrier.id;
  return catalog.providers?.some((provider) => provider.id === carrier.id)
    ? carrier.id
    : family.fallbackCarrierId;
}

function selectedCarrierId(family, catalog) {
  const explicit = explicitCarrierId(family, catalog);
  if (explicit) return explicit;
  const defaultCarrier = family.carriers.find(
    (carrier) => carrier.id === catalog?.default_provider,
  );
  if (!defaultCarrier) return family.fallbackCarrierId;
  if (!defaultCarrier.requiresProviderCatalog) return defaultCarrier.id;
  return catalog.providers?.some((provider) => provider.id === defaultCarrier.id)
    ? defaultCarrier.id
    : family.fallbackCarrierId;
}

function flatCatalogProviderId(defaultProviderId) {
  const family = providerFamilyForCarrier(defaultProviderId);
  const carrier = family?.carriers.find(({ id }) => id === defaultProviderId);
  return carrier?.requiresProviderCatalog
    ? family.fallbackCarrierId
    : defaultProviderId || STARTABLE_PROVIDERS[0].id;
}

export function normalizeModelCatalog(catalog) {
  if (catalog?.providers?.length) return { ...catalog };
  const id = flatCatalogProviderId(catalog?.default_provider);
  return {
    default_provider: id,
    providers: [{
      id,
      label: providerLabel(id),
      models: catalog?.models || [],
      efforts: catalog?.efforts || [],
    }],
  };
}

export function catalogForProvider(catalog, providerId) {
  const providers = catalog.providers || [];
  return providers.find((provider) => provider.id === providerId) || providers[0] || { models: [], efforts: [] };
}

export function creatableCatalog(catalog) {
  const servedProviders = (catalog && catalog.providers) || [];
  const providers = PROVIDER_FAMILIES.map((family) => {
    const id = selectedCarrierId(family, catalog);
    const servedProvider = servedProviders.find((provider) => provider.id === id) || {};
    return {
      id,
      label: family.genericLabel,
      models: servedProvider.models || [],
      efforts: servedProvider.efforts || [],
    };
  });
  return { ...catalog, providers };
}
