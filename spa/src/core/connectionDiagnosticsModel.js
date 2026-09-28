// What the connection diagnostic history says, as rows a person can read.
//
// The history itself (core/connectionDiagnostics.js) is written for a console:
// a flat list of `{ at, connection, event, ...detail }`, oldest first, where
// `connection` is the diagnostic id a peer link was opened under and everything
// past `event` is whatever that event had to say. deploy/OPS.md has asked
// people to run `buildConnectionDiagnostics()` and read it themselves — which
// is no help at all on a phone, where there is no console to run it in.
//
// So: newest first, the machine named rather than identified, and the detail as
// one compact line. Pure, so the shape of a row is settled without a DOM.

import { deviceNameOf } from "./devicePolicy.js";

/// What the local stores record their events under: the replica cache
/// (core/localCache.js) and the UI store (core/localUiStore.js). Named here
/// rather than imported so this pure model does not load either.
const LOCAL_DIAGNOSTICS = new Set(["local-cache", "local-ui-store"]);

/// The two fields every entry has, which are therefore never detail.
const ENVELOPE_FIELDS = new Set(["at", "connection", "event"]);

/// The machine a diagnostic is about, read off the id it was recorded under.
///
/// Every id is minted as `<deviceId>:<sessionId>` (core/connection.js and
/// terminal/session.js are the two that mint them), and device ids are uuids,
/// so the first colon is the seam. An id in any other shape names no machine —
/// better to say nothing than to name the wrong one.
export function deviceIdOfDiagnostic(connection) {
  const seam = typeof connection === "string" ? connection.indexOf(":") : -1;
  return seam > 0 ? connection.slice(0, seam) : "";
}

/// Enough of an id to tell two of them apart at a glance, for a machine the
/// account list has never heard of — one that was revoked while this tab was
/// open, or a diagnostic recorded before the list had answered.
export const shortDeviceId = (deviceId) => (deviceId ? `${deviceId.slice(0, 8)}…` : "unknown device");

/// What the account calls the machine, falling back to the short id. The name
/// is the whole point of the row: a phone screen has no room to correlate uuids
/// by eye.
export function diagnosticDeviceLabel(connection, devices) {
  // The local stores' events are about this browser, not about any machine.
  if (LOCAL_DIAGNOSTICS.has(connection)) return "this browser";
  const deviceId = deviceIdOfDiagnostic(connection);
  return deviceNameOf(devices, deviceId) || shortDeviceId(deviceId);
}

/// The local wall-clock time, to the second. Seconds are the point: these
/// events arrive in bursts, and a minute's resolution would pile a whole
/// reconnect into one timestamp. A row whose `at` is not a number says so
/// rather than rendering "Invalid Date".
export function formatDiagnosticTime(at) {
  const when = new Date(Number(at));
  if (Number.isNaN(when.getTime())) return "—";
  return when.toLocaleTimeString(undefined, { hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/// Everything the event carried, as `key=value` in the order it was recorded.
/// Compact on purpose: it goes on one monospace line that scrolls sideways, and
/// a phone gives it about forty characters before it has to.
export function diagnosticDetailText(entry) {
  return Object.entries(entry || {})
    .filter(([field]) => !ENVELOPE_FIELDS.has(field))
    .map(([field, value]) => `${field}=${formatDetailValue(value)}`)
    .join(" ");
}

/// A detail value as one word. Objects are rare here and shallow when they
/// happen, so JSON is both honest and short enough.
function formatDetailValue(value) {
  if (value === null || value === undefined) return "";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

/// One history entry as a row: when, which machine, which connection, what
/// happened, and what it said.
///
/// `source` is the diagnostic id, or the channel for an entry recorded without
/// one — the column answers "which connection is this about", and those are the
/// two things that can answer it.
function diagnosticRow(entry, devices, index) {
  return {
    key: `${entry.at}:${index}`,
    at: Number(entry.at) || 0,
    time: formatDiagnosticTime(entry.at),
    device: diagnosticDeviceLabel(entry.connection, devices),
    source: entry.connection || entry.channel || "",
    kind: entry.event || "",
    detail: diagnosticDetailText(entry),
  };
}

/// The history as rows, newest first — the order a person reads a fault in,
/// because what just happened is what they are looking at the screen about.
///
/// The history is recorded oldest-first and append-only, so reversing the
/// indexed list is the whole of it; entries are not re-sorted by `at`, because
/// recording order is the truth and a clock that stepped should not reorder
/// what actually happened.
export function diagnosticRows(history, devices = []) {
  return (history || [])
    .map((entry, index) => diagnosticRow(entry || {}, devices, index))
    .reverse();
}

/// The dump that gets copied and shared: the raw events, untouched, so what
/// support reads is what deploy/OPS.md describes and not this module's reading of
/// them — wrapped in the two facts a reader cannot recover from the events alone.
///
/// `since` is when this tab started recording and `dropped` is how many events it
/// had to discard to make room. Both exist because the report that prompted #60
/// carried only the final session: the failures had been pushed out of the ring by
/// the storm they caused, and nothing in the paste said so, so it read as a
/// complete account of a connection that had simply worked.
///
/// An array is still accepted, for a caller that has only the events.
export function diagnosticsJson(report) {
  const whole = Array.isArray(report) ? { events: report } : report || {};
  return JSON.stringify(
    {
      since: whole.since ?? null,
      dropped: whole.dropped ?? 0,
      events: whole.events || [],
    },
    null,
    2,
  );
}

/// What the rows amount to, for the line above them. A count rather than a
/// verdict: this section reports, it does not diagnose.
export const diagnosticsSummary = (rows) =>
  rows.length ? `${rows.length} recorded event${rows.length === 1 ? "" : "s"}, newest first.` : "";
