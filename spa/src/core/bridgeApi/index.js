// Which adapter speaks to the bridge on the other end.
//
// Neither end is ever assumed current. The bridge reports `api_version` in its
// greeting; this SPA carries one adapter per API major it knows and picks by
// range. A gap in either direction is not a broken surface, it is a gate:
//
//   bridge above every adapter's range → the app is behind  ("app")
//   bridge below every adapter's floor → the bridge is behind ("bridge")
//
// `unsupported` names the side that needs updating, which is the side each
// gate in views/versionGate.js speaks about.

import { compare, satisfies } from "./semver.js";
import * as v1 from "./v1/index.js";

export { ApiError, normalizeError, UNKNOWN_CODE } from "./v1/index.js";
export { compare, parse, satisfies } from "./semver.js";

/** A bridge that reports no `api_version` is pre-alpha. Every shipped bridge
 *  reports one; the value stands for a greeting fixture or a refused hello,
 *  and the lowest adapter takes it with every capability flag off. */
export const PRE_ALPHA_API_VERSION = "0.0.0";

/** Every major this build speaks, lowest first. */
export const ADAPTERS = Object.freeze([v1]);

/** The range this SPA declares in `session.hello`, spanning every adapter. */
export const SPA_API_RANGE = ">=2.0.0 <4.0.0";

/** The version a greeting reports — `0.0.0` for one that reports none, or for
 *  no greeting at all (a bridge that refused `session.hello`). */
export function greetingVersion(greeting) {
  const reported = greeting?.api_version;
  return typeof reported === "string" && reported ? reported : PRE_ALPHA_API_VERSION;
}

/** The lowest version a range admits: its `>=` bound, or 0.0.0 for a range
 *  that names none. */
function floorOf(range) {
  const match = /(?:^|\s)>=\s*(\d+\.\d+\.\d+)/.exec(String(range || ""));
  return match ? match[1] : "0.0.0";
}

/** A version no adapter admits is on one side or the other: under every
 *  floor, the bridge is behind; otherwise the app is. */
function unsupportedSide(version, adapters) {
  const belowEvery = adapters.every((adapter) => compare(version, floorOf(adapter.range)) < 0);
  return belowEvery ? "bridge" : "app";
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
  const wanted = version === PRE_ALPHA_API_VERSION ? floorOf(adapters[0]?.range) : version;
  const adapter = adapters.find((candidate) => satisfies(wanted, candidate.range));
  if (!adapter) return { unsupported: unsupportedSide(version, adapters), version };
  return {
    major: adapter.major,
    range: adapter.range,
    version,
    create: (call) => adapter.create(call, greeting),
  };
}
