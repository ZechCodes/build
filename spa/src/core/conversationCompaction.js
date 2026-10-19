// Conversation settings paint configured intent separately from applied policy.
import { compactionObservationCopy, compactionTokenLabel, inheritedCompactionLabel, positiveCompactionTokens } from "./compactionPresentation.js";

export const COMPACT_OPTION_PREFIX = "compact:";
const LIMITS = [null, 150000, 200000, 300000];
const isCustomLimit = (limit) => positiveCompactionTokens(limit) && !LIMITS.includes(limit);
const optionIdOf = (limit) => `${COMPACT_OPTION_PREFIX}${limit === null ? "default" : limit}`;
const ownLimitOf = (agent) => agent?.max_context_tokens || null;
const thresholdWord = (tokens) => tokens % 1000 === 0 ? `${tokens / 1000}k` : compactionTokenLabel({ tokens });

export function compactionMenuOptions(agent) {
  const standing = ownLimitOf(agent);
  const limits = isCustomLimit(standing) ? [...LIMITS, standing] : LIMITS;
  return limits.map((limit) => ({
    id: optionIdOf(limit),
    label: limit === null ? inheritedCompactionLabel(agent?.compaction?.inherited)
      : isCustomLimit(limit) ? `Custom (${thresholdWord(limit)})` : thresholdWord(limit),
    selected: limit === standing,
  }));
}

export function compactionMenuGroup(agent) {
  return { id: "compact", label: "Compact at", control: "slider", options: compactionMenuOptions(agent),
    description: compactionObservationCopy(agent?.compaction) };
}

/** The limit a custom row's id stands for, or null for any other id. Read
 *  back only safe integers written exactly as `optionIdOf` writes them. */
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
 * The slider holds its committed value until the cache confirms or changes
 * the threshold. The cache changes only when the bridge answers or pushes.
 * A choice while one is in flight is ignored, as a second press of the switch is.
 * `choose` resolves when the answer has been offered to the
 * cache or the verb has been refused, and never rejects — a refusal is
 * `onFailure`'s. It resolves true for an accepted choice, false for a refusal
 * or an ignored choice during another save, so the slider can roll back.
 */
export function createCompactionChoice({ call, capture, write, onFailure = () => {}, onSaved = () => {} }) {
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
      if (pending || (maxContextTokens !== null && !positiveCompactionTokens(maxContextTokens))) return false;
      if (ownLimitOf(agent) === maxContextTokens) return true;
      pending = true;
      try {
        const captured = await capture(entityId).catch(() => null);
        const reply = await ask(entityId, agent, maxContextTokens);
        if (!reply) return false;
        onSaved();
        if (!captured) return true;
        const fields = answeredFields(reply);
        // The bridge has it once it answers: a cache that cannot take the
        // answer is not a refusal, and the push brings the same word.
        await write(captured, agent.id, (held) => (sameCompaction(held, fields) ? null : mergeCompactionReply(held, fields)))
          .catch(() => {});
        return true;
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
  if (reply.compaction !== undefined) fields.compaction = reply.compaction;
  return fields;
}

const sameCompaction = (agent, fields) =>
  Object.entries(fields).every(([field, value]) => (agent[field] ?? null) === value);

// The settings reply describes inheritance, not a newly observed active turn.
function mergeCompactionReply(held, fields) {
  const next = { ...held, ...fields };
  if (fields.compaction) next.compaction = { ...held.compaction, ...fields.compaction };
  return next;
}
