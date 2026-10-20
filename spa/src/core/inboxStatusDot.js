/** The inbox's one status mark: colour says unread activity, and a pulse says
 * an agent is running. With neither, the row needs no mark. Both facts come
 * from the row's cached model, independently of one another. */
function statusLabel({ running, unread, descendantUnread }) {
  const unreadLabel = unread ? "Unread activity" : descendantUnread ? "A worker has unread messages" : "";
  return [running ? "Running" : "", unreadLabel].filter(Boolean).join(" · ");
}

export function statusDotHtml({ running = false, unread = false, descendantUnread = false } = {}) {
  const hasUnread = unread || descendantUnread;
  if (!running && !hasUnread) return "";
  const classes = ["inbox-status-dot", running ? "inbox-status-running" : "", hasUnread ? "inbox-status-unread" : ""]
    .filter(Boolean).join(" ");
  const label = statusLabel({ running, unread, descendantUnread });
  return `<span class="${classes}" role="img" aria-label="${label}" title="${label}"></span>`;
}
