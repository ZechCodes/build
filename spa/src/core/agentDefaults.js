// The agent, model and reasoning effort a new issue starts with.
//
// Filing an issue is the common act; picking a harness for it is the rare one.
// So the sheet hides those three behind an advanced panel and starts them here,
// and this is what the Account page edits. An empty string means "whatever the
// bridge's catalog says is default" — a preference should be able to say
// "no preference", or the client would pin a model the daemon has since dropped.
//
// Browser-scoped (localStorage), like every other preference this client keeps:
// the device picker, read state, the collapsed rail.

import { genericProviderId } from "./modelPicker.js";

export const AGENT_DEFAULTS_KEY = "build.agentDefaults";

const EMPTY = { provider: "", model: "", effort: "" };

const clean = (value) => (typeof value === "string" ? value : "");

/** The stored defaults, or empties. Never throws: a corrupt or unreadable value
 *  reads as "no preference", which is the same thing the app did before it had
 *  any preferences at all.
 *
 *  A provider saved when the picker offered a card per carrier reads as the
 *  agent that carries it — the preference always meant "Claude Code", and which
 *  program that opens is the account's answer now, not this browser's. */
export function loadAgentDefaults(storage = localStorage) {
  try {
    const parsed = JSON.parse(storage.getItem(AGENT_DEFAULTS_KEY) || "{}");
    if (!parsed || typeof parsed !== "object") return { ...EMPTY };
    return {
      provider: genericProviderId(clean(parsed.provider)),
      model: clean(parsed.model),
      effort: clean(parsed.effort),
    };
  } catch {
    return { ...EMPTY };
  }
}

/** Persist the defaults, keeping only the three known fields. Returns what was
 *  stored so a caller can render it without re-reading. */
export function saveAgentDefaults(next, storage = localStorage) {
  const value = {
    provider: clean(next && next.provider),
    model: clean(next && next.model),
    effort: clean(next && next.effort),
  };
  try {
    storage.setItem(AGENT_DEFAULTS_KEY, JSON.stringify(value));
  } catch {
    /* private mode: the session keeps working, the preference just does not stick */
  }
  return value;
}

/** A model belongs to its provider, and an effort to its model — so changing the
 *  provider drops a model chosen under the old one, and dropping the model drops
 *  the effort with it. Pure, so both the sheet and the Account panel agree. */
export function reconcileAgentDefaults(defaults, { providerChanged = false, modelChanged = false } = {}) {
  const next = { ...EMPTY, ...defaults };
  if (providerChanged) {
    next.model = "";
    next.effort = "";
  } else if (modelChanged) {
    next.effort = "";
  }
  return next;
}
