/** The inbox's one status mark: colour says unread activity, and a pulse says
 * an agent is running. With neither, the row needs no mark. Both facts come
 * from the row's cached model, independently of one another. */
export function statusDotHtml({ running = false, unread = false } = {}) {
  if (!running && !unread) return "";
  const classes = ["inbox-status-dot", running ? "inbox-status-running" : "", unread ? "inbox-status-unread" : ""]
    .filter(Boolean).join(" ");
  const label = [running ? "Running" : "", unread ? "Unread activity" : ""].filter(Boolean).join(" · ");
  return `<span class="${classes}" role="img" aria-label="${label}" title="${label}"></span>`;
}
