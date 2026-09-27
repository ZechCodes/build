// The issue timeline's adapter for the same visit marker used by chat.
// Persisted comment/event ids share a ULID clock after their kind prefix.
const persistedId = /^(?:ic|ie)-([^-]+)$/;
export const issueUnreadKey = (id) => persistedId.exec(id || "")?.[1] ?? null;
const compare = (left, right) => left < right ? -1 : left > right ? 1 : 0;

/** Compare a comment mark and an event mark by their shared ULID clock. */
export function latestIssueMark(left, right) {
  const earlier = issueUnreadKey(left);
  const later = issueUnreadKey(right);
  if (earlier === null) return later === null ? null : right;
  return later !== null && compare(later, earlier) > 0 ? right : left;
}

/** The events that are news to the user (#183): a change to who holds the
 *  issue or where it stands, or an agent-created issue that asks the user to
 *  read it. Comments count too; other bookkeeping does not. The bridge's
 *  `unread_count` reads the same list (bridge/src/app/tracker/inbox.rs
 *  `counts_as_unread`), and spa/test/issueUnreadKinds183.test.js holds the
 *  two to the cases the bridge prints. */
const NEWS_EVENT_KINDS = new Set(["assigned", "unassigned", "moved", "closed", "reopened"]);
const isAskedCreation = (row) => row.kind === "created" && row.mentionsUser === true && row.actor?.kind === "agent";
const isNews = (row) => row.type === "comment" || (row.type === "event" && (NEWS_EVENT_KINDS.has(row.kind) || isAskedCreation(row)));

export const issueUnreadRules = {
  eligible: (row) => row.actor?.kind !== "user" && isNews(row),
  key: (row) => issueUnreadKey(row.key),
  valid: (key) => typeof key === "string",
  compare,
  first: "",
};

export function issueUnreadReading(rows, mark, readThrough = "") {
  const cursor = issueUnreadKey(mark) ?? "";
  const items = rows || [];
  return {
    cursor,
    items,
    unreadCount: items.filter((row) => issueUnreadRules.eligible(row) &&
      issueUnreadRules.valid(issueUnreadRules.key(row)) &&
      compare(issueUnreadRules.key(row), cursor) > 0).length,
    readThrough: issueUnreadKey(readThrough) ?? "",
  };
}
