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

export const COMPACT_OPTION_PREFIX = "compact:";

/** What the rows ask for, in the order they stand. `null` is the device's
 *  default and 0 is never — the wire's own words for both. */
const LIMITS = [null, 150000, 200000, 300000, 0];

/** A limit of the conversation's own that none of the offered rows sends —
 *  set elsewhere, by another client or the bridge's own tooling. Any positive
 *  whole number the digest carries: the wire's u64 arrives as whatever Number
 *  the JSON parse made of it, and that same Number is what the row is checked
 *  against, so one past 2^53 is no less a limit. */
const isCustomLimit = (limit) => Number.isInteger(limit) && limit > 0 && !LIMITS.includes(limit);

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
  size: (_agent, limit) => ({ word: thresholdWord(limit), description: sizeDescription(limit) }),
  custom: (_agent, limit) => ({ word: `Custom (${thresholdWord(limit)})`, description: sizeDescription(limit) }),
};

function sizeDescription(limit) {
  return `Compact once a turn fills ${thresholdWord(limit)} tokens of context`;
}

function copyKindOf(limit) {
  if (limit === null) return "default";
  if (limit === 0) return "off";
  return isCustomLimit(limit) ? "custom" : "size";
}

/** The limits the rows stand for: the offered ones, and a limit set elsewhere
 *  on a row of its own ahead of never, so one row is always the checked one. */
function limitsFor(agent) {
  const standing = ownLimitOf(agent);
  if (!isCustomLimit(standing)) return LIMITS;
  const never = LIMITS.length - 1;
  return [...LIMITS.slice(0, never), standing, ...LIMITS.slice(never)];
}

/** The menu's rows: one per limit, the standing one marked. */
export function compactionMenuOptions(agent) {
  const standing = ownLimitOf(agent);
  return limitsFor(agent).map((limit) => {
    const { word, description } = ROW_COPY[copyKindOf(limit)](agent, limit);
    return { id: optionIdOf(limit), label: word, description, selected: limit === standing };
  });
}

/** The menu's group: discrete stops under the setting's name
 *  (core/splitButton.js `groupedMenuButtonMarkup`). */
export function compactionMenuGroup(agent) {
  return { id: "compact", label: "Compact at", control: "slider", options: compactionMenuOptions(agent) };
}

/** The limit a custom row's id stands for, or null for any other id. Read
 *  back only where the id is exactly what `optionIdOf` writes for it — which
 *  for a limit past 1e21 is the exponent form `String` gives a Number. */
function customLimitOfOptionId(id) {
  if (!id.startsWith(COMPACT_OPTION_PREFIX)) return null;
  const written = id.slice(COMPACT_OPTION_PREFIX.length);
  const limit = Number(written);
  return isCustomLimit(limit) && String(limit) === written ? limit : null;
}

/** The limit a menu id stands for, as `{ maxContextTokens }`, or null for an id
 *  that is not one of these rows. Wrapped because null is itself a limit. */
export function compactionLimitOfOptionId(optionId) {
  const id = String(optionId || "");
  const offered = LIMITS.find((each) => optionIdOf(each) === id);
  if (offered !== undefined) return { maxContextTokens: offered };
  const custom = customLimitOfOptionId(id);
  return custom === null ? null : { maxContextTokens: custom };
}

/** `conversation.settings`'s params, in the fixture's shape. */
export function compactionSettingsParams(entityId, agentId, maxContextTokens, threadId) {
  return { entity_id: entityId, agent_id: agentId, max_context_tokens: maxContextTokens, ...(threadId ? { thread_id: threadId } : {}) };
}

/**
 * Choosing when a conversation compacts, and writing what the bridge answered
 * into the cache the rail paints from.
 *
 * The answer lands only where the agent's row has not been written since the
 * verb went out (`capture` before, `write` after, compared inside the cache's
 * own transaction). Nothing on the wire orders a push against a reply: the
 * bridge builds a push's row under its lock, lets go, and sends the row later,
 * so a row read before the change can reach the device after the answer to
 * it. A row carries no revision to tell the two apart. What the bridge does
 * promise is that the change itself is noted, so a push carrying the new limit
 * follows; a row written in between — stale or newer, from a push or another
 * tab — keeps its place, and that push settles it. Nothing is held and nothing
 * is written twice, so no answer can land over a newer value or chase another
 * tab's answer through the shared cache.
 *
 * `capture(entityId)` answers the row's write as it stands; `write(captured,
 * agentId, rewrite)` lays `rewrite(agent)` over the agent while the row still
 * holds that write, and `rewrite` answers null to leave it alone.
 *
 * Not optimistic, unlike the watch switch (core/watchToggle.js): the menu has
 * shut by the time a choice is sent, so there is no control under the finger
 * to move early. A choice while one is in flight is ignored, as a second press
 * of the switch is. `choose` resolves when the answer has been offered to the
 * cache or the verb has been refused, and never rejects — a refusal is
 * `onFailure`'s.
 */
export function createCompactionChoice({ call, capture, write, onFailure = () => {} }) {
  let pending = false;

  const ask = async (entityId, agent, maxContextTokens) => {
    try {
      return await call("conversation.settings", compactionSettingsParams(entityId, agent.id, maxContextTokens, agent.thread_id));
    } catch (error) {
      onFailure(error);
      return null;
    }
  };

  return {
    async choose({ entityId, agent, maxContextTokens }) {
      if (pending || ownLimitOf(agent) === maxContextTokens) return;
      pending = true;
      try {
        const captured = await capture(entityId).catch(() => null);
        const reply = await ask(entityId, agent, maxContextTokens);
        if (!reply || !captured) return;
        const fields = answeredFields(reply);
        // The bridge has it once it answers: a cache that cannot take the
        // answer is not a refusal, and the push brings the same word.
        await write(captured, agent.id, (held) => (sameCompaction(held, fields) ? null : { ...held, ...fields }))
          .catch(() => {});
      } finally {
        pending = false;
      }
    },
  };
}

/** What `conversation.settings` answered, as the digest's own fields. */
function answeredFields(reply) {
  const fields = { max_context_tokens: reply.max_context_tokens ?? null };
  if (reply.compact_at_tokens !== undefined) fields.compact_at_tokens = reply.compact_at_tokens;
  return fields;
}

const sameCompaction = (agent, fields) =>
  Object.entries(fields).every(([field, value]) => (agent[field] ?? null) === value);
