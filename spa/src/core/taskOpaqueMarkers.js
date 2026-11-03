// Canonical bridge task-marker semantics, intentionally separate from the
// existing unread/attention adapters. Source: tracker/summary.rs when(),
// after(), watch_started_after(), and app/tracker/tracking.rs read-through.
// Inputs are complete marker IDs, never already-extracted suffixes. Null and
// undefined model Option::None; every string, including "", is present.
// No prefix, ULID, whitespace, NUL, normalization, or byte-length policy is
// imposed here. The caller owns admission bounds. Rust strings are valid
// Unicode; JS-only unpaired surrogates are rejected instead of replaced.

const utf8 = new TextEncoder();

export const isPresentTaskMarker = (marker) => typeof marker === "string";

/** Rust when(): retain the full remainder after the first hyphen, or the
 *  complete original string if there is no hyphen. Empty suffixes are valid. */
export function taskMarkerSuffix(marker) {
  if (!isPresentTaskMarker(marker) || !marker.isWellFormed()) {
    throw new TypeError("A task marker must be a well-formed Unicode string");
  }
  const separator = marker.indexOf("-");
  return separator < 0 ? marker : marker.slice(separator + 1);
}

function compareUtf8(left, right) {
  const leftBytes = utf8.encode(left);
  const rightBytes = utf8.encode(right);
  const shared = Math.min(leftBytes.length, rightBytes.length);
  for (let index = 0; index < shared; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) return leftBytes[index] < rightBytes[index] ? -1 : 1;
  }
  return leftBytes.length < rightBytes.length ? -1 : leftBytes.length > rightBytes.length ? 1 : 0;
}

/** Compare complete optional marker IDs by Rust UTF-8 suffix order.
 *  Absence sorts before every present marker, even an empty one. */
export function compareTaskMarkers(left, right) {
  if (left == null) {
    if (right == null) return 0;
    taskMarkerSuffix(right);
    return -1;
  }
  if (right == null) {
    taskMarkerSuffix(left);
    return 1;
  }
  return compareUtf8(taskMarkerSuffix(left), taskMarkerSuffix(right));
}

/** Rust after(mark, id): None admits every entry, Some compares strictly. */
export const taskMarkerAfter = (mark, id) => mark == null || compareTaskMarkers(id, mark) > 0;

/** Inclusive read-through coverage (Rust when(read) >= when(event_id)). */
export const taskMarkerCovers = (mark, id) => mark != null && compareTaskMarkers(mark, id) >= 0;

/** Preserve the complete newer marker; equal suffixes retain the left input.
 *  This tie convention does not infer identity from equal ordering keys. */
export function newerTaskMarker(left, right) {
  const newer = compareTaskMarkers(left, right) < 0 ? right : left;
  return newer ?? null;
}

/** One task's watch floor, not a cache-record reconciliation policy. A present
 *  stored floor bypasses fallback, including "". Legacy fallback uses the last
 *  maximal watched event, matching Rust Iterator::max_by_key's equal-key rule.
 *  Timeline entries use wire IDs (`id`), not rendered row keys (`key`). */
export function effectiveTaskWatchFloor(task, timeline = []) {
  const persisted = task?.watch_started_after;
  if (persisted != null) {
    taskMarkerSuffix(persisted);
    return persisted;
  }
  let floor = null;
  for (const entry of timeline) {
    if (entry.type === "event" && entry.kind === "watched" && compareTaskMarkers(entry.id, floor) >= 0) {
      floor = entry.id;
    }
  }
  return floor;
}
