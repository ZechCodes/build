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

/** Marks an agent in the cache as carrying what `conversation.settings`
 *  answered rather than what a digest said. A push rewrites the row whole, so
 *  the mark is gone the moment the bridge's own word arrives. */
const ANSWERED_MARK = "__compactionAnswered";

/**
 * The rail's hold on what it asked for, per agent.
 *
 * What the bridge answers is written into the cached agent (`write`), and the
 * rail repaints off that write as it does off any other. The push that carries
 * the new limit can land after the reply, and one built before the change can
 * land after it too: a row taken up (`takeUp`) whose agent still says the old
 * limit has the answer written back over it, and the repaint that follows puts
 * the tick back on the answer. (The stale row is painted once in between; the
 * menu has shut by then, so what shows it is the gauge, for one frame.) Once
 * a digest of the bridge's own — unmarked — says the same, the digest is the
 * truth again, so a change made elsewhere shows.
 *
 * `write(entityId, agentId, rewrite)` lays `rewrite(agent)` over the agent
 * wherever the rail reads it; `rewrite` answers null to leave it alone.
 *
 * Not optimistic, unlike the watch switch (core/watchToggle.js): the menu has
 * shut by the time a choice is sent, so there is no control under the finger
 * to move early. A choice while one is in flight is ignored, as a second press
 * of the switch is. `choose` resolves when the answer is in the cache or the
 * verb has been refused, and never rejects — a refusal is `onFailure`'s.
 */
export function createCompactionChoice({ call, write, onFailure = () => {} }) {
  const answered = new Map(); // agent id → { entityId, fields } conversation.settings answered
  let pending = false;

  /** Whether an agent's digest is the bridge's own word agreeing with the
   *  answer, which ends the hold. */
  const confirms = (agent, answer) =>
    ownLimitOf(agent) === answer.fields.max_context_tokens && agent[ANSWERED_MARK] !== true;

  const writeAnswer = async (agentId, answer) => {
    let confirmed = false;
    await write(answer.entityId, agentId, (agent) => {
      confirmed = confirms(agent, answer);
      if (ownLimitOf(agent) === answer.fields.max_context_tokens) return null;
      return { ...agent, ...answer.fields, [ANSWERED_MARK]: true };
    });
    if (confirmed && answered.get(agentId) === answer) answered.delete(agentId);
  };

  return {
    /** A row the rail has just stood on: its agents, as the cache holds them. */
    takeUp(agents) {
      for (const agent of agents || []) {
        const answer = agent && answered.get(agent.id);
        if (!answer) continue;
        if (confirms(agent, answer)) answered.delete(agent.id);
        else if (ownLimitOf(agent) !== answer.fields.max_context_tokens) void writeAnswer(agent.id, answer);
      }
    },

    async choose({ entityId, agent, maxContextTokens }) {
      if (pending || ownLimitOf(agent) === maxContextTokens) return;
      pending = true;
      try {
        let reply;
        try {
          reply = await call("conversation.settings", compactionSettingsParams(entityId, agent.id, maxContextTokens));
        } catch (error) {
          onFailure(error);
          return;
        }
        const fields = { max_context_tokens: reply?.max_context_tokens ?? null };
        if (reply?.compact_at_tokens !== undefined) fields.compact_at_tokens = reply.compact_at_tokens;
        const answer = { entityId, fields };
        answered.set(agent.id, answer);
        // The bridge has it: a cache that cannot take the answer is not a
        // refusal, and the push brings the same word.
        await writeAnswer(agent.id, answer).catch(() => {});
      } finally {
        pending = false;
      }
    },
  };
}
