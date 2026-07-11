// Pure text helpers shared by every surface.

export const esc = (value) =>
  (value ?? "").toString().replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Human-scale age: <60s "just now", <1h "Nm ago", <1d "Nh ago", else "Nd ago". */
export function humanAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
