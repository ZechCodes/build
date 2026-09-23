// Inbox sessions are computed from compact, cached conversation spans. A span
// begins after at least twelve hours of silence in one conversation; merging
// spans across conversations gives the session of a workspace or project.

export const SESSION_GAP_MS = 12 * 60 * 60 * 1000;

/** The newest session in the union of several conversations. `null` means an
 * older bridge did not supply the metadata, while an empty array means the
 * bridge knows that no message has been sent. */
export function newestSession(conversations) {
  if (!Array.isArray(conversations) || conversations.some((conversation) => !Array.isArray(conversation.activity_spans))) return null;
  const spans = conversations.flatMap((conversation) => conversation.activity_spans || [])
    .filter((span) => Array.isArray(span) && span.length === 2
      && span.every(Number.isSafeInteger) && span[0] <= span[1])
    .sort((left, right) => left[0] - right[0] || left[1] - right[1]);
  if (!spans.length) return { anchorMs: null, lastActivityMs: null };
  let [start, end] = spans[0];
  for (const [nextStart, nextEnd] of spans.slice(1)) {
    if (nextStart - end < SESSION_GAP_MS) end = Math.max(end, nextEnd);
    else [start, end] = [nextStart, nextEnd];
  }
  return { anchorMs: start, lastActivityMs: end };
}
