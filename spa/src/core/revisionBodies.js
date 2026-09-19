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

const revisions = new Map();

/** One revision's contents, read once per session. The promise is what is
 *  held, so two presses on the same chip make one call. */
export function revisionContents(entityId, revisionId, call) {
  const key = `${entityId || ""}|${revisionId}`;
  const held = revisions.get(key);
  if (held) return held;
  const reading = Promise.resolve(call("thread.revision", { entity_id: entityId, revision_id: revisionId }))
    .catch((error) => {
      // A failure is not a fact about the revision: the next press asks again.
      if (revisions.get(key) === reading) revisions.delete(key);
      throw error;
    });
  revisions.set(key, reading);
  return reading;
}

/** Forget them. For tests, and for a session teardown — what was read belongs
 *  to the person who was signed in. */
export const forgetRevisionBodies = () => revisions.clear();
