// The compact fields of wire 3.26 TaskSummaryView. Full action responses may
// carry bodies, attachments, dependency expansions and other link collections.
// None of that content belongs in the normalized list cache.
const fields = `id project_id number title state status labels priority assignee created_by
created_at updated_at closed_at done_at watched trackers attachment_count summary_truncated
read_through watch_started_after dismissed_through unread_count unread_agent_comments unread_mentions
session_started_ms last_activity_ms review_summary deferred_assignment`.split(/\s+/);
const pick = (value, keys) => Object.fromEntries(keys.filter((key) => Object.hasOwn(value, key)).map((key) => [key, value[key]]));

/** Omitted compact fields survive an action patch. This deliberately does not
 * turn absent expanded dependency/link arrays into empty authoritative facts. */
export function summaryPatch(held, incoming) {
  const next = { ...held, ...pick(incoming, fields) };
  for (const [key, keys] of [["links", ["workspace_ids", "parent_task_id"]],
    ["dependencies", ["blocked", "waiting_on_count", "blocks_count"]]]) {
    if (incoming[key]) next[key] = { ...held?.[key], ...pick(incoming[key], keys) };
  }
  return next;
}
