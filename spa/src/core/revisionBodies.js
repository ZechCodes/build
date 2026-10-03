// The contents of a diff revision a conversation points at.
//
// The two bodies a conversation points at rather than carries — an attachment's
// bytes, and the contents of a diff revision — are content-addressed and
// immutable, and both are far too big to be worth a record: the cache is what a
// reader opening a workspace paints from, and nobody opens a workspace to look
// at a revision they last read a week ago.
//
// So they are fetched when the reader asks for them and held in memory for the
// session. An attachment's bytes live on the conversation's own state
// (core/thread.js `createThreadState`, which the repository keeps across
// remounts); a revision has nowhere of its own, so it lives here, keyed by the
// entity it belongs to.

import { conversationContentKey, matchesConversationContent } from "./conversationContentScope.js";

const revisions = new Map();

/** One revision's contents, read once per session. The promise is what is
 *  held, so two presses on the same chip make one call. */
export function revisionContents(entityId, revisionId, call, ownership = {}) {
  const scope = { ...ownership, entityId };
  const key = `${conversationContentKey(scope)}|${revisionId}`;
  const held = revisions.get(key);
  if (held) return held.reading;
  const params = { entity_id: entityId, revision_id: revisionId,
    ...(scope.agentId ? { agent_id: scope.agentId } : {}),
    ...(scope.conversationId ? { conversation_id: scope.conversationId } : {}),
    ...(scope.threadId ? { thread_id: scope.threadId } : {}),
  };
  const reading = Promise.resolve(call("thread.revision", params))
    .then((body) => {
      if (revisions.get(key)?.reading !== reading) throw new Error("Conversation was cleared while reading its revision");
      return body;
    })
    .catch((error) => {
      // A failure is not a fact about the revision: the next press asks again.
      if (revisions.get(key)?.reading === reading) revisions.delete(key);
      throw error;
    });
  revisions.set(key, { reading, scope });
  return reading;
}

/** Forget them. For tests, and for a session teardown — what was read belongs
 *  to the person who was signed in. */
export const forgetRevisionBodies = () => revisions.clear();

/** Remove held bodies and fence reads still crossing the wire at reset. */
export function forgetConversationRevisionBodies(ownership) {
  for (const [key, held] of revisions) {
    if (matchesConversationContent(held.scope, ownership)) revisions.delete(key);
  }
}
