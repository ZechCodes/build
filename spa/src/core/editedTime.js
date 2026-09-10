const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const RELATIVE_LIMIT_MS = 7 * DAY_MS;

export function editedTimestamp(value) {
  if (value === null || value === undefined || value === "") return null;
  const timestamp = Number(value);
  if (!Number.isFinite(timestamp)) return null;
  return Number.isFinite(new Date(timestamp).valueOf()) ? timestamp : null;
}

/** The compact age shown beside a file's diffstat. */
export function editedTimeLabel(editedAt, now = Date.now()) {
  const timestamp = editedTimestamp(editedAt);
  if (timestamp === null) return "";
  const elapsed = Math.max(0, now - timestamp);
  if (elapsed < MINUTE_MS) return "Just Now";
  if (elapsed < HOUR_MS) return ago(Math.floor(elapsed / MINUTE_MS), "minute");
  if (elapsed < DAY_MS) return ago(Math.floor(elapsed / HOUR_MS), "hour");
  if (elapsed > RELATIVE_LIMIT_MS) return new Date(timestamp).toISOString().slice(0, 10);
  const days = Math.floor(elapsed / DAY_MS);
  return ago(days, "day");
}

const ago = (count, unit) => `${count} ${unit}${count === 1 ? "" : "s"} ago`;

/** Refresh only timestamp text. File nodes and their interactive state survive. */
export function refreshEditedTimes(root, now = Date.now()) {
  for (const element of root.querySelectorAll("[data-edited-at]")) {
    const label = editedTimeLabel(element.dataset.editedAt, now);
    if (element.textContent !== label) element.textContent = label;
  }
}

export function watchEditedTimes(root, intervalMs = 30_000) {
  refreshEditedTimes(root);
  const timer = setInterval(() => refreshEditedTimes(root), intervalMs);
  return { dispose: () => clearInterval(timer) };
}
