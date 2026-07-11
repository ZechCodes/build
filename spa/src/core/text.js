// Pure text helpers shared by every surface.

// Quotes are escaped too: esc() output is interpolated into attribute values
// (data-dir="...", data-tab="...") where an unescaped quote from an untrusted
// name (e.g. a repo filename) would inject live attributes.
export const esc = (value) =>
  (value ?? "")
    .toString()
    .replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Human-scale age: <60s "just now", <1h "Nm ago", <1d "Nh ago", else "Nd ago". */
export function humanAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
