// The model and reasoning effort each harness starts with, and the harness a
// new issue or agent starts on.
//
// Filing an issue is the common act; picking a harness for it is the rare one.
// So the sheet hides that choice behind an advanced panel and starts it here,
// and this is what the Account page edits. An empty model or effort means
// "whatever the harness's own config says" — a preference should be able to
// say "no preference", or the client would pin a model the daemon has since
// dropped.
//
// A preference belongs to a harness FAMILY (Claude Code, Codex): both carriers
// of a family run the same CLI over the same catalog, so a model chosen for
// Claude Code is the model for Claude Code whichever carrier the device's agent
// modes pick today. A harness outside every family is keyed by its own name.
//
// Browser-scoped (localStorage), like every other preference this client keeps:
// the device picker, read state, the collapsed rail.

import { chosenProviderId } from "./agentChoice.js";
import { creatableCatalog, providerFamilyKey } from "./providerCatalog.js";

export const AGENT_DEFAULTS_KEY = "build.agentDefaults";

const NO_PREFERENCE = Object.freeze({ model: "", effort: "" });

const clean = (value) => (typeof value === "string" ? value : "");
const isObject = (value) => !!value && typeof value === "object" && !Array.isArray(value);

const cleanPreference = (entry) => ({ model: clean(entry?.model), effort: clean(entry?.effort) });

const cleanHarnesses = (harnesses) =>
  Object.fromEntries(
    Object.entries(isObject(harnesses) ? harnesses : {}).map(([key, entry]) => [key, cleanPreference(entry)]),
  );

/** What storage holds, as an object, or nothing: a corrupt or unreadable value
 *  reads as "no preference", which is what the app did before it had any. */
function readStored(storage) {
  try {
    const parsed = JSON.parse(storage.getItem(AGENT_DEFAULTS_KEY) || "{}");
    return isObject(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

/** Every harness's stored preference and the harness a start leads with:
 *  `{ provider, harnesses: { [family]: { model, effort } } }`. Never throws.
 *
 *  A value written before there was one preference per harness held a single
 *  trio; it reads as that harness's preference, so nothing anyone chose is lost
 *  to the upgrade. */
export function loadHarnessDefaults(storage = localStorage) {
  const stored = readStored(storage);
  const provider = clean(stored.provider);
  if ("harnesses" in stored) return { provider, harnesses: cleanHarnesses(stored.harnesses) };
  const harnesses = provider ? { [providerFamilyKey(provider)]: cleanPreference(stored) } : {};
  return { provider, harnesses };
}

/** The model and effort stored for `providerId`'s harness, or no preference. */
export function harnessDefaultsFor(defaults, providerId) {
  return defaults.harnesses[providerFamilyKey(providerId)] || { ...NO_PREFERENCE };
}

/** The choice a start on `providerId` leads with: that harness, and its own
 *  stored model and effort. */
export function agentDefaultsFor(providerId, storage = localStorage) {
  return { provider: clean(providerId), ...harnessDefaultsFor(loadHarnessDefaults(storage), providerId) };
}

/** The stored defaults as one choice: the default harness with its own model
 *  and effort. Nothing is chosen until a harness is, since a model belongs to
 *  its harness; a surface that knows what the catalog offers asks
 *  `agentDefaultsIn` instead and gets the catalog's default harness resolved. */
export function loadAgentDefaults(storage = localStorage) {
  const defaults = loadHarnessDefaults(storage);
  return { provider: defaults.provider, ...harnessDefaultsFor(defaults, defaults.provider) };
}

/** The choice a surface over `catalog` starts with: the stored harness, clamped
 *  onto what the catalog offers the way every create surface clamps it, and
 *  that harness's stored model and effort — so a preference for the catalog's
 *  own default applies even while no harness was ever chosen. */
export function agentDefaultsIn(catalog, storage = localStorage) {
  const defaults = loadHarnessDefaults(storage);
  const provider = chosenProviderId(creatableCatalog(catalog || {}), defaults);
  return { provider, ...harnessDefaultsFor(defaults, provider) };
}

function persist(defaults, storage) {
  const value = { provider: clean(defaults.provider), harnesses: cleanHarnesses(defaults.harnesses) };
  try {
    storage.setItem(AGENT_DEFAULTS_KEY, JSON.stringify(value));
  } catch {
    /* private mode: the session keeps working, the preference just does not stick */
  }
  return value;
}

/** Make `providerId` the harness a start leads with. Every harness's own
 *  preference stays as it was. */
export function saveDefaultHarness(providerId, storage = localStorage) {
  return persist({ ...loadHarnessDefaults(storage), provider: providerId }, storage);
}

/** Store the model and effort `providerId`'s harness starts with, keeping only
 *  the two known fields. Returns what was stored so a caller can render it
 *  without re-reading. */
export function saveHarnessDefault(providerId, preference, storage = localStorage) {
  const defaults = loadHarnessDefaults(storage);
  const harnesses = { ...defaults.harnesses, [providerFamilyKey(providerId)]: cleanPreference(preference) };
  return persist({ ...defaults, harnesses }, storage);
}
