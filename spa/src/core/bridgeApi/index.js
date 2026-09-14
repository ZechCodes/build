// Which adapter speaks to the bridge on the other end.
//
// Neither end is ever assumed current. The bridge reports `api_version` in its
// greeting; this SPA carries one adapter per API major it knows and picks by
// range. A gap in either direction is not a broken surface, it is a gate:
//
//   bridge major above every adapter's range → the app is behind  ("app")
//   bridge major below every adapter's range → the bridge is behind ("bridge")
//
// `unsupported` names the side that needs updating, which is the side each
// gate in views/versionGate.js speaks about.

import { satisfies } from "./semver.js";
import * as v1 from "./v1/index.js";

export { ApiError, normalizeError, UNKNOWN_CODE } from "./v1/index.js";
export { compare, parse, satisfies } from "./semver.js";

/** A bridge that reports no `api_version` is pre-alpha. The v1 adapter takes
 *  it, with every capability flag off. */
export const PRE_ALPHA_API_VERSION = "0.0.0";

/** Every major this build speaks, lowest first. */
export const ADAPTERS = Object.freeze([v1]);

/** The range this SPA declares in `session.hello`, spanning every adapter. */
export const SPA_API_RANGE = ">=1.0.0 <2.0.0";

/** The version a greeting reports — `0.0.0` for one that reports none, or for
 *  no greeting at all (a bridge that refused `session.hello`). */
export function greetingVersion(greeting) {
  const reported = greeting?.api_version;
  return typeof reported === "string" && reported ? reported : PRE_ALPHA_API_VERSION;
}

function majorOf(version) {
  return Number(String(version).split(".")[0]) || 0;
}

/** Pre-alpha is not "below every adapter" — the v1 adapter claims it below. */
function unsupportedSide(version, adapters) {
  const lowest = Math.min(...adapters.map((adapter) => adapter.major));
  return majorOf(version) < lowest ? "bridge" : "app";
}

/**
 * The adapter for this greeting, or which side is out of date.
 *
 * @returns `{ major, range, version, create(call) }` — `create` is the major's
 *   own `create(call, greeting)` with this greeting already bound — or
 *   `{ unsupported: "bridge" | "app", version }`.
 */
export function selectAdapter(greeting, adapters = ADAPTERS) {
  const version = greetingVersion(greeting);
  const wanted = version === PRE_ALPHA_API_VERSION ? "1.0.0" : version;
  const adapter = adapters.find((candidate) => satisfies(wanted, candidate.range));
  if (!adapter) return { unsupported: unsupportedSide(version, adapters), version };
  return {
    major: adapter.major,
    range: adapter.range,
    version,
    create: (call) => adapter.create(call, greeting),
  };
}
