// What a new PROJECT agent starts on: the account's harness defaults with this
// device's own project-agent choice laid over the top.
//
// The account page (core/harnessDefaults.js) says what every start leads with,
// and a workspace lays its own choice over that for the work in it
// (core/workspaceDefaults.js). The project agent is neither: it is the one
// agent that talks ABOUT a project rather than working in a checkout, and the
// harness that suits that job is often not the one coding work leads with. So
// it gets a slot of its own on the Local settings page, and the two layer
// rather than compete.
//
// There is exactly ONE slot, not one per project: this is a device preference
// about a kind of agent, the way the account's own defaults are.
//
// LAYERING, field by field, and STORAGE, browser-local under a key of its own:
// both exactly as `core/workspaceDefaults.js` does them, and for the same
// reason — the slot is shaped like the account's own value, which is what lets
// `projectAgentDefaultsStorage` hand it to the account's loaders and to the
// account's panel untouched.

import { chosenProviderId } from "./agentChoice.js";
import { harnessDefaultsFor, layerHarnessDefaults, loadHarnessDefaults } from "./agentDefaults.js";
import { creatableCatalog } from "./providerCatalog.js";

export const PROJECT_AGENT_DEFAULTS_KEY = "build.projectAgentDefaults";

/** A Storage-shaped view of the project agent's slot.
 *
 *  Only the two methods `core/agentDefaults.js` uses are implemented, and only
 *  for its own key: this is not a Storage, it is the adapter that lets the
 *  account's loaders, savers and settings panel read and write the project
 *  agent's preference without knowing there is such a thing. */
export function projectAgentDefaultsStorage(storage = localStorage) {
  return {
    getItem: () => storage.getItem(PROJECT_AGENT_DEFAULTS_KEY) || "{}",
    setItem: (_key, value) => {
      try {
        storage.setItem(PROJECT_AGENT_DEFAULTS_KEY, value);
      } catch {
        /* private mode: the session keeps working, the preference just does
           not stick */
      }
    },
  };
}

/** The account's harness defaults with the project agent's laid over them. */
export function layeredProjectAgentDefaults(storage = localStorage) {
  return layerHarnessDefaults(
    loadHarnessDefaults(storage),
    loadHarnessDefaults(projectAgentDefaultsStorage(storage)),
  );
}

/** The harness, model and effort a new project agent starts on, with no
 *  catalog to resolve against: whatever was chosen, and empties where nothing
 *  was — so a device that says nothing leaves the bridge's own default harness
 *  standing. This is what the project page sends on
 *  `project.ensure_conversation`. */
export function projectAgentChoice(storage = localStorage) {
  const defaults = layeredProjectAgentDefaults(storage);
  return { provider: defaults.provider, ...harnessDefaultsFor(defaults, defaults.provider) };
}

/** `agentDefaultsFor`, layered: the model and effort a project agent on
 *  `providerId` starts with. */
export function projectAgentDefaultsFor(providerId, storage = localStorage) {
  const provider = typeof providerId === "string" ? providerId : "";
  return { provider, ...harnessDefaultsFor(layeredProjectAgentDefaults(storage), provider) };
}

/** `agentDefaultsIn`, layered: which harness a new project agent leads with —
 *  clamped onto what the catalog actually offers, the way every create surface
 *  clamps it — and that harness's own model and effort. */
export function projectAgentDefaultsIn(catalog, storage = localStorage) {
  const defaults = layeredProjectAgentDefaults(storage);
  const provider = chosenProviderId(creatableCatalog(catalog || {}), defaults);
  return { provider, ...harnessDefaultsFor(defaults, provider) };
}
