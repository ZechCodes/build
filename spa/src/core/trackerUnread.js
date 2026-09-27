// The task timeline's adapter for the same visit marker used by chat.
// Persisted comment/event ids share a ULID clock after their kind prefix.
const persistedId = /^(?:ic|ie)-([^-]+)$/;
export const taskUnreadKey = (id) => persistedId.exec(id || "")?.[1] ?? null;
const compare = (left, right) => left < right ? -1 : left > right ? 1 : 0;

/** Compare a comment mark and an event mark by their shared ULID clock. */
export function latestTaskMark(left, right) {
  const earlier = taskUnreadKey(left);
  const later = taskUnreadKey(right);
  if (earlier === null) return later === null ? null : right;
  return later !== null && compare(later, earlier) > 0 ? right : left;
}

/** The events that are news to the user (#183): a change to who holds the
 *  task or where it stands, or an agent-created task that asks the user to
 *  read it. Comments count too; other bookkeeping does not. The bridge's
 *  `unread_count` reads the same list (bridge/src/app/tracker/inbox.rs
 *  `counts_as_unread`), and spa/test/taskUnreadKinds183.test.js holds the
 *  two to the cases the bridge prints. */
const NEWS_EVENT_KINDS = new Set(["assigned", "unassigned", "moved", "closed", "reopened"]);
const isAskedCreation = (row) => row.kind === "created" && row.mentionsUser === true && row.actor?.kind === "agent";
const isNews = (row) => row.type === "comment" || (row.type === "event" && (NEWS_EVENT_KINDS.has(row.kind) || isAskedCreation(row)));

export const taskUnreadRules = {
  eligible: (row) => row.actor?.kind !== "user" && isNews(row),
  key: (row) => taskUnreadKey(row.key),
  valid: (key) => typeof key === "string",
  compare,
  first: "",
};

export function taskUnreadReading(rows, mark, readThrough = "") {
  const cursor = taskUnreadKey(mark) ?? "";
  const items = rows || [];
  return {
    cursor,
    items,
    unreadCount: items.filter((row) => taskUnreadRules.eligible(row) &&
      taskUnreadRules.valid(taskUnreadRules.key(row)) &&
      compare(taskUnreadRules.key(row), cursor) > 0).length,
    readThrough: taskUnreadKey(readThrough) ?? "",
  };
}
