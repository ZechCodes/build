// Pure text helpers shared by every surface.

// Quotes are escaped too: esc() output is interpolated into attribute values
// (data-dir="...", data-tab="...") where an unescaped quote from an untrusted
// name (e.g. a repo filename) would inject live attributes.
export const esc = (value) =>
  (value ?? "")
    .toString()
    .replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** What a refusal says on the surface that asked. A thrown Error carries the
 *  words; anything else a call rejected with is shown as it reads. */
export const messageOf = (error) => (error instanceof Error ? error.message : String(error));

/** What the account says about the one machine it had and lost: honest about
 *  WHEN it went unreachable (a moving "reconnecting…" claim reads as a lie while
 *  nothing is happening) and calm about what resumes automatically. */
export function deviceUnreachableText(name, sinceMs) {
  const time = new Date(sinceMs).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return `${name || "Your device"} unreachable since ${time} — tasks will resume when it reconnects.`;
}

/** What a work surface says when the machine its link names cannot answer —
 *  gone offline, or never opened on this client: there is nothing to read and
 *  nothing to write until that machine is back. */
export const deviceOfflineText = (name) =>
  `${name || "That device"} isn't connected, so this can't be opened right now.`;

/** What a work surface says when the machine it is about goes while it is open:
 *  it keeps what it read, so the only thing missing is whose state that is. */
export const deviceFrozenText = (name) =>
  `${name || "That device"} isn't connected — this is what it last said.`;

/** What it says where nothing is reachable: no device to name, and no time that
 *  would mean anything, so it says what is true and what happens. */
export const allDevicesOfflineText = () =>
  "All devices are offline — tasks will resume when one reconnects.";

/** Human-scale age: <60s "just now", <1h "Nm ago", <1d "Nh ago", else "Nd ago". */
export function humanAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
