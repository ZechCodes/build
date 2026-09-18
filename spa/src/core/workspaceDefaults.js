// What a new agent starts on inside ONE workspace: the account's harness
// defaults with this workspace's own choices laid over the top.
//
// The account page (core/harnessDefaults.js) says what every start leads with;
// a workspace is where you are actually working, and the work in it often
// wants something else — a cheap model for a workspace you are only reading,
// the reasoning level a thorny migration needs. So the workspace gets the same
// panel, scoped to itself, and the two layer rather than compete.
//
// LAYERING, field by field: an empty model or effort is "no opinion" at either
// level, exactly as it is on the account page, so a workspace can name a model
// without being made to restate the effort beside it — and a workspace that
// names nothing behaves precisely as it did before it had a sheet.
//
// STORAGE: browser-local, like every other preference this client keeps, under
// one key holding a slot per workspace. The slot is shaped exactly like the
// account's own value, which is what lets `workspaceDefaultsStorage` hand it to
// the account's loaders and to the account's panel untouched — the workspace
// half of this feature is a storage adapter, not a second implementation.

import { chosenProviderId } from "./agentChoice.js";
import { agentDefaultsFor, agentDefaultsIn, harnessDefaultsFor, loadHarnessDefaults } from "./agentDefaults.js";
import { creatableCatalog } from "./providerCatalog.js";

export const WORKSPACE_DEFAULTS_KEY = "build.workspaceDefaults";

const isObject = (value) => !!value && typeof value === "object" && !Array.isArray(value);

/** Every workspace's slot, as an object. A corrupt or unreadable value reads as
 *  "no workspace named anything", which is what the app did before it had
 *  workspace defaults at all. */
function readSlots(storage) {
  try {
    const parsed = JSON.parse(storage.getItem(WORKSPACE_DEFAULTS_KEY) || "{}");
    return isObject(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

/** A Storage-shaped view of one workspace's slot.
 *
 *  Only the two methods `core/agentDefaults.js` uses are implemented, and only
 *  for its own key: this is not a Storage, it is the adapter that lets the
 *  account's loaders, savers and settings panel read and write a workspace's
 *  preference without knowing there is such a thing. A key of null is a
 *  workspace-less surface, and reads as empty and writes nowhere. */
export function workspaceDefaultsStorage(workspaceKey, storage = localStorage) {
  return {
    getItem: () => (workspaceKey ? JSON.stringify(readSlots(storage)[workspaceKey] || {}) : "{}"),
    setItem: (_key, value) => {
      if (!workspaceKey) return;
      try {
        const slots = readSlots(storage);
        slots[workspaceKey] = JSON.parse(value);
        storage.setItem(WORKSPACE_DEFAULTS_KEY, JSON.stringify(slots));
      } catch {
        /* private mode, or a value that will not parse: the session keeps
           working, the preference just does not stick */
      }
    },
  };
}

/** Drop a workspace's slot — what a deleted workspace leaves behind, so an id
 *  the bridge will never mint again stops taking up room. */
export function forgetWorkspaceDefaults(workspaceKey, storage = localStorage) {
  if (!workspaceKey) return;
  try {
    const slots = readSlots(storage);
    delete slots[workspaceKey];
    storage.setItem(WORKSPACE_DEFAULTS_KEY, JSON.stringify(slots));
  } catch {
    /* private mode: nothing was stored to forget */
  }
}

/** The account's harness defaults with this workspace's own laid over them.
 *  Outside a workspace it is the account's, unchanged. */
export function layeredHarnessDefaults(workspaceKey, storage = localStorage) {
  const account = loadHarnessDefaults(storage);
  if (!workspaceKey) return account;
  const workspace = loadHarnessDefaults(workspaceDefaultsStorage(workspaceKey, storage));
  const harnesses = { ...account.harnesses };
  for (const [family, preference] of Object.entries(workspace.harnesses)) {
    const under = harnesses[family] || { model: "", effort: "" };
    harnesses[family] = { model: preference.model || under.model, effort: preference.effort || under.effort };
  }
  return { provider: workspace.provider || account.provider, harnesses };
}

/** `agentDefaultsFor`, layered: the model and effort a start on `providerId`
 *  leads with while standing in this workspace. */
export function agentDefaultsForWorkspace(workspaceKey, providerId, storage = localStorage) {
  if (!workspaceKey) return agentDefaultsFor(providerId, storage);
  const provider = typeof providerId === "string" ? providerId : "";
  return { provider, ...harnessDefaultsFor(layeredHarnessDefaults(workspaceKey, storage), provider) };
}

/** `agentDefaultsIn`, layered: which harness a start in this workspace leads
 *  with — clamped onto what the catalog actually offers, the way every create
 *  surface clamps it — and that harness's own model and effort. */
export function agentDefaultsInWorkspace(workspaceKey, catalog, storage = localStorage) {
  if (!workspaceKey) return agentDefaultsIn(catalog, storage);
  const defaults = layeredHarnessDefaults(workspaceKey, storage);
  const provider = chosenProviderId(creatableCatalog(catalog || {}), defaults);
  return { provider, ...harnessDefaultsFor(defaults, provider) };
}
