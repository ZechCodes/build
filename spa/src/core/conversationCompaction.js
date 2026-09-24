// When one conversation compacts, as the ⋮ on its head offers it.
//
// The bridge compacts an agent's session before its next turn once the last
// turn's context reached a threshold (wire 1.10). The device holds a default
// (`compact_above_tokens`); a conversation may carry a limit of its own, set
// with `conversation.settings`. The agent digest says both: `max_context_tokens`
// is the conversation's own limit (null or absent when it follows the device)
// and `compact_at_tokens` the threshold in effect, 0 when it never compacts.
//
// The rows sit in the same menu as the detail levels, and are routed the same
// way: by a prefix on their ids (core/conversationDetail.js). They stand as a
// group of their own under the setting's name, so each row is the bare value.

import { bridgeCapabilities } from "./changeEvents.js";

export const COMPACT_OPTION_PREFIX = "compact:";

/** What the rows ask for, in the order they stand. `null` is the device's
 *  default and 0 is never — the wire's own words for both. */
const LIMITS = [null, 150000, 200000, 300000, 0];

/** A row's id, by the limit it sends. */
const optionIdOf = (limit) => {
  if (limit === null) return `${COMPACT_OPTION_PREFIX}default`;
  if (limit === 0) return `${COMPACT_OPTION_PREFIX}off`;
  return `${COMPACT_OPTION_PREFIX}${limit}`;
};

/** A threshold the way a person reads one: "200k", or "off" for never. */
const thresholdWord = (tokens) => (tokens === 0 ? "off" : `${Math.round(tokens / 1000)}k`);

/** The conversation's own limit off its digest, null when it follows the
 *  device — which is what a digest from before the field says too. */
const ownLimitOf = (agent) => agent?.max_context_tokens ?? null;

/** The default row names the threshold it would give, and it can only know
 *  that while no limit of the conversation's own stands in front of it. */
function defaultRowLabel(agent) {
  const threshold = agent?.compact_at_tokens;
  if (ownLimitOf(agent) !== null || !Number.isFinite(threshold)) return "Default";
  return `Default (${thresholdWord(threshold)})`;
}

const ROW_COPY = {
  default: (agent) => ({ word: defaultRowLabel(agent), description: "This device's setting" }),
  off: () => ({ word: "Off", description: "Never compact this chat" }),
  size: (_agent, limit) => ({
    word: thresholdWord(limit),
    description: `Compact once a turn fills ${thresholdWord(limit)} tokens of context`,
  }),
};

function copyKindOf(limit) {
  if (limit === null) return "default";
  return limit === 0 ? "off" : "size";
}

/** The menu's rows: one per limit, the standing one marked. */
export function compactionMenuOptions(agent) {
  const standing = ownLimitOf(agent);
  return LIMITS.map((limit) => {
    const { word, description } = ROW_COPY[copyKindOf(limit)](agent, limit);
    return { id: optionIdOf(limit), label: word, description, selected: limit === standing };
  });
}

/** The menu's group: the limits as a radio set under the setting's name
 *  (core/splitButton.js `groupedMenuButtonMarkup`). */
export function compactionMenuGroup(agent) {
  return { id: "compact", label: "Compact at", options: compactionMenuOptions(agent) };
}

/** The limit a menu id stands for, as `{ maxContextTokens }`, or null for an id
 *  that is not one of these rows. Wrapped because null is itself a limit. */
export function compactionLimitOfOptionId(optionId) {
  const limit = LIMITS.find((each) => optionIdOf(each) === String(optionId || ""));
  return limit === undefined ? null : { maxContextTokens: limit };
}

/** `conversation.settings`'s params, in the fixture's shape. */
export function compactionSettingsParams(entityId, agentId, maxContextTokens) {
  return { entity_id: entityId, agent_id: agentId, max_context_tokens: maxContextTokens };
}

/** Whether this device's bridge carries `conversation.settings` (1.10). A
 *  row wired to a verb an older bridge has never heard of can only refuse. */
export function carriesCompactionSettings(deviceId) {
  return bridgeCapabilities(deviceId)?.conversations?.settings === true;
}

/**
 * The rail's hold on what it asked for, per agent.
 *
 * What the bridge answers is shown until the digest says the same: the push
 * that carries the new limit can land after the reply, and a menu read off the
 * older digest in between would put the tick back where it was. Once the two
 * agree the digest is the truth again, so a change made elsewhere shows.
 *
 * Not optimistic, unlike the watch switch (core/watchToggle.js): the menu has
 * shut by the time a choice is sent, so there is no control under the finger
 * to move early. A choice while one is in flight is ignored, as a second press
 * of the switch is. `choose` resolves when the verb has settled or been
 * refused and never rejects — a refusal is `onFailure`'s.
 */
export function createCompactionChoice({ call, onSettled = () => {}, onFailure = () => {} }) {
  const answered = new Map(); // agent id → what conversation.settings answered
  let pending = false;

  const agentAsKnown = (agent) => {
    const answer = agent && answered.get(agent.id);
    if (!answer) return agent;
    if (ownLimitOf(agent) === answer.max_context_tokens) {
      answered.delete(agent.id);
      return agent;
    }
    return { ...agent, ...answer };
  };

  return {
    agentAsKnown,

    async choose({ entityId, agent, maxContextTokens }) {
      if (pending || ownLimitOf(agentAsKnown(agent)) === maxContextTokens) return;
      pending = true;
      try {
        const answer = await call("conversation.settings", compactionSettingsParams(entityId, agent.id, maxContextTokens));
        answered.set(agent.id, {
          max_context_tokens: answer?.max_context_tokens ?? null,
          compact_at_tokens: answer?.compact_at_tokens,
        });
        onSettled();
      } catch (error) {
        onFailure(error);
      } finally {
        pending = false;
      }
    },
  };
}
