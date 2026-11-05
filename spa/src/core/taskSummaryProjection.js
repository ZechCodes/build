// The compact fields of wire 3.26 TaskSummaryView. Full action responses may
// carry bodies, attachments, dependency expansions and other link collections.
// None of that content belongs in the normalized list cache.
const fields = `id project_id number title state status labels priority assignee created_by
created_at updated_at closed_at done_at watched trackers attachment_count summary_truncated
read_through watch_started_after dismissed_through unread_count unread_agent_comments unread_mentions unread_service_mentions
session_started_ms last_activity_ms review_summary legacy_review deferred_assignment last_reviewed_snapshot_id`.split(/\s+/);
const pick = (value, keys) => Object.fromEntries(keys.filter((key) => Object.hasOwn(value, key)).map((key) => [key, value[key]]));

/** Omitted compact fields survive an action patch. This deliberately does not
 * turn absent expanded dependency/link arrays into empty authoritative facts. */
export function summaryPatch(held, incoming, fullTask = false) {
  const defaults = fullTask ? { watched: false, done_at: null, review_summary: null, deferred_assignment: null,
    attachment_count: incoming.attachments?.length || 0 } : {};
  const next = { ...held, ...defaults, ...pick(incoming, fields) };
  delete next.__review_summary_cache;
  for (const [key, keys] of [["links", ["workspace_ids", "parent_task_id"]],
    ["dependencies", ["blocked", "waiting_on_count", "blocks_count"]]]) {
    if (incoming[key]) next[key] = { ...held?.[key], ...pick(incoming[key], keys) };
  }
  return next;
}

/** A full TaskView is a different wire shape from a summary. Project only
 * that response's ordinary fields: its complete waiting_on array proves the
 * waiting count, while blocks can be paged and use their explicit count.
 * summary_truncated belongs to TaskSummaryView and has no full-view oracle. */
export function fullTaskSummary(incoming) {
  const next = summaryPatch(undefined, incoming, true);
  delete next.summary_truncated;
  if (Array.isArray(incoming.attachments)) next.attachment_count = incoming.attachments.length;
  else delete next.attachment_count;
  if (next.dependencies) {
    delete next.dependencies.waiting_on_count;
    if (Array.isArray(incoming.dependencies?.waiting_on)) next.dependencies.waiting_on_count = incoming.dependencies.waiting_on.length;
  }
  return next;
}
