// Wire 3.11's conversation.reset capability also announces generation-aware
// requests. Cached conversation identities may come from a newer bridge, so
// the greeting at dispatch determines which fields can go over this session.
const generationFields = new Map([
  ["thread.attach", ["agent_id", "thread_id"]],
  ["thread.attachment", ["agent_id", "conversation_id", "thread_id"]],
  ["thread.revision", ["agent_id", "conversation_id", "thread_id"]],
  ["thread.page", ["thread_id"]],
  ["thread.activity", ["thread_id"]],
  ["thread.post", ["thread_id"]],
  ["agent.start", ["thread_id"]],
  ["agent.interrupt", ["thread_id"]],
  ["agent.choose", ["thread_id"]],
  ["entity.seen", ["thread_id"]],
  ["conversation.settings", ["thread_id"]],
  ["conversation.watch", ["thread_id"]],
  ["conversation.unwatch", ["thread_id"]],
]);

/** Restore the pre-3.11 shape without mutating a caller's held identity or
 * removing established selectors, paging fields, or unrelated parameters. */
export function legacyThreadParams(method, params) {
  const fields = generationFields.get(method);
  if (!params || !fields || !fields.some((field) => Object.hasOwn(params, field))) return params;
  return Object.fromEntries(Object.entries(params).filter(([field]) => !fields.includes(field)));
}
