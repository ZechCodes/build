export function timeAgo(isoOrTs) {
  if (!isoOrTs) return 'never';
  const date = typeof isoOrTs === 'number'
    ? new Date(isoOrTs * 1000)
    : new Date(isoOrTs);
  const seconds = Math.floor((Date.now() - date.getTime()) / 1000);
  if (seconds < 5) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function shortTime(isoOrTs) {
  if (!isoOrTs) return '';
  const date = typeof isoOrTs === 'number'
    ? new Date(isoOrTs * 1000)
    : new Date(isoOrTs);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

export function fmtRelativeAgo(ageMs) {
  const s = Math.max(0, Math.floor(ageMs / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

export function fmtClock24(date) {
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/**
 * Activity-entry label format: no "ago" suffix.
 *   < 5s    → "now"
 *   < 60s   → "Ns"
 *   < 60m   → "Nm"
 *   >= 1h   → wall-clock hh:mm
 */
export function relativeOrClock(isoOrTs) {
  if (!isoOrTs) return '';
  const ts = typeof isoOrTs === 'number'
    ? (isoOrTs < 1e12 ? isoOrTs * 1000 : isoOrTs)
    : Date.parse(isoOrTs);
  if (!ts || Number.isNaN(ts)) return '';
  const seconds = Math.floor((Date.now() - ts) / 1000);
  if (seconds < 5) return 'now';
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return fmtClock24(new Date(ts));
}
