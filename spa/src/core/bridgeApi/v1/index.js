// The v1 adapter: everything this SPA knows about API major 1.
//
// A surface never asks what version the bridge reports — it asks the adapter's
// `capabilities`, derived from the names in the greeting. Only bridges before
// 1.22 use the legacy minor table; feature branches no longer bump a version.
// The floor is 1.2: the client reads what
// a push carries and polls nothing, and a bridge below 1.2 pushes keys, not
// bodies, so it is gated rather than served a client that would never move.
//
// The adapter also owns error normalisation. From 1.1 a refusal carries
// `error_code`, `retryable` and `details` beside the string; from 1.0 it is
// the string alone, which becomes `ApiError("unknown")` with the text intact.
// A view therefore reads `error.code` whatever it is talking to.

import { satisfies } from "../semver.js";

/** The range of bridge versions this adapter claims. */
export const range = ">=1.2.0 <2.0.0";

/** The API major it is the adapter for. */
export const major = 1;

/** `ApiError.code` for a refusal that named none — a 1.0 bridge's string. */
export const UNKNOWN_CODE = "unknown";

/** The closed set of codes a 1.x bridge refuses with (`api/mod.rs`), plus the
 *  client-side stand-in for a refusal that named none. Additive within a major:
 *  an unrecognised code still arrives on the ApiError as it was sent. */
export const ERROR_CODES = Object.freeze([
  "unknown_method",
  "invalid_params",
  "not_found",
  "conflict",
  "unavailable",
  "busy",
  "unsupported_version",
  "internal",
  UNKNOWN_CODE,
]);

/** Every push a 1.x bridge sends on a session: the change events, the terminal
 *  frames and the signalling one. An event of any other type is a no-op, never
 *  a throw — a later bridge may add one. `fixtures/api/v1/events.json` carries
 *  one example of each, and both ends are held to it. */
export const EVENT_TYPES = Object.freeze([
  "board.changed",
  "entity.changed",
  "changes",
  "term.output",
  "term.reset",
  "term.closed",
  "rtc.ice",
  "bridge.update_status",
]);

/** What a bridge that pushes but names no event list sends: the legacy pair. */
const LEGACY_EVENTS = Object.freeze(["board.changed", "entity.changed"]);

/** A refusal, whichever shape it arrived in. */
export class ApiError extends Error {
  constructor(code, message, { retryable = false, details = {}, cause, timedOut = false, uncertain = false } = {}) {
    super(message || code);
    this.name = "ApiError";
    this.code = code;
    this.retryable = retryable;
    this.details = details;
    this.timedOut = timedOut;
    this.uncertain = uncertain;
    if (cause !== undefined) this.cause = cause;
  }
}

function messageOf(thrown) {
  if (typeof thrown === "string" && thrown) return thrown;
  if (thrown instanceof Error && thrown.message) return thrown.message;
  if (thrown && typeof thrown.error === "string" && thrown.error) return thrown.error;
  return "the call failed";
}

function fieldsOf(thrown) {
  const source = thrown && typeof thrown === "object" ? thrown : {};
  const details = source.details && typeof source.details === "object" ? source.details : {};
  return {
    code: typeof source.error_code === "string" && source.error_code ? source.error_code : UNKNOWN_CODE,
    retryable: source.retryable === true,
    details,
    timedOut: source.timedOut === true,
    uncertain: source.uncertain === true,
  };
}

/**
 * Whatever a call rejected with — an Error the transport threw, a raw refusal
 * reply, a bare string — as an ApiError. An ApiError is returned untouched, so
 * a normaliser in a retry loop never double-wraps.
 */
export function normalizeError(thrown) {
  if (thrown instanceof ApiError) return thrown;
  const fields = fieldsOf(thrown);
  return new ApiError(fields.code, messageOf(thrown), {
    ...fields,
    cause: thrown instanceof Error ? thrown : undefined,
  });
}

/** A result off the wire. Unknown fields are kept, not stripped: forward
 *  compatibility is the rule on both ends, and a caller reads what it knows. */
export function parseResult(method, result) {
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    throw new TypeError(`${method}: result is not an object`);
  }
  return result;
}

/** A push off the session, or null for one this major does not know. */
export function parseEvent(event) {
  if (!event || typeof event !== "object") return null;
  return EVENT_TYPES.includes(event.type) ? event : null;
}

/** Names this client understands, with the historical minor and (where one
 *  existed) explicit greeting flag. This table is frozen history: new features
 *  get names, never a new minor fallback. */
const LEGACY_CAPABILITIES = Object.freeze([
  { name: "changes.subscriptions", minor: 1, flag: ["changes", "subscriptions"] },
  { name: "requests.priority", minor: 1, flag: ["requests", "priority"] },
  { name: "errors.codes", minor: 1, flag: ["errors", "codes"] },
  { name: "diffs.perFile", minor: 4 },
  { name: "issues.context", minor: 5 },
  { name: "issues.attachments", minor: 8, flag: ["issues", "attachments"] },
  { name: "issues.watching", minor: 9, flag: ["issues", "watching"] },
  { name: "conversations.settings", minor: 10 },
  { name: "messages.context", offered: (greeting) => greeting.message_context?.version === 1 },
  {
    name: "threads.postOperations",
    offered: (greeting) => greeting.thread_post_operations?.version === 1
      && typeof greeting.thread_post_operations.status_method === "string",
  },
]);

function legacyNames(greeting, version) {
  const minor = Number(version.split(".")[1]);
  return LEGACY_CAPABILITIES.filter(({ minor: floor, flag, offered }) => {
    if (offered) return offered(greeting);
    const stated = flag && greeting[flag[0]]?.[flag[1]];
    return typeof stated === "boolean" ? stated : minor >= floor;
  }).map(({ name }) => name);
}

/** An absent or malformed list on 1.22+ claims nothing. On older bridges only,
 *  the historical booleans and minor supply names when there is no list. */
function namesOf(greeting, version) {
  if (typeof greeting?.api_version !== "string" || !satisfies(version, ">=1.0.0 <2.0.0")) return new Set();
  if (Object.hasOwn(greeting, "capabilities")) {
    return new Set(Array.isArray(greeting.capabilities) ? greeting.capabilities : []);
  }
  return new Set(satisfies(version, ">=1.0.0 <1.22.0") ? legacyNames(greeting, version) : []);
}

/** The existing surface-facing flags, selected independently by name. Unknown
 *  names never become flags. Subscription kinds and push event names continue
 *  to come from their own advertised lists. */
export function capabilitiesOf(greeting, version = greeting?.api_version || "0.0.0") {
  const names = namesOf(greeting, version);
  const subscriptions = names.has("changes.subscriptions");
  const kinds = greeting?.changes?.kinds;
  return {
    changes: {
      subscriptions,
      kinds: subscriptions && Array.isArray(kinds) ? kinds.filter((kind) => typeof kind === "string") : [],
    },
    requests: { priority: names.has("requests.priority") },
    errors: { codes: names.has("errors.codes") },
    diffs: { perFile: names.has("diffs.perFile") },
    issues: {
      context: names.has("issues.context"),
      attachments: names.has("issues.attachments"),
      watching: names.has("issues.watching"),
      doneSinceLeft: names.has("issues.doneSinceLeft"),
      commentUserNotifies: names.has("issues.commentUserNotifies"),
    },
    conversations: { settings: names.has("conversations.settings") },
    messages: { context: names.has("messages.context") },
    threads: { postOperations: names.has("threads.postOperations") },
  };
}

function eventsOf(greeting) {
  if (Array.isArray(greeting?.events)) return greeting.events.filter((name) => typeof name === "string");
  return greeting?.push_events === true ? [...LEGACY_EVENTS] : [];
}

/**
 * Bind a session's `call` to this major.
 *
 * @param call `(method, params, options?) => Promise<result>` — the session's
 *   own rpc, which may reject with an Error or (defensively) resolve a raw
 *   `{ok:false}` reply. Either becomes an ApiError.
 * @param greeting the `session.hello` reply this adapter was selected for.
 */
export function create(call, greeting) {
  const reported = greeting?.api_version;
  const version = typeof reported === "string" && reported ? reported : "0.0.0";
  // Forwarded verbatim, arity and all: the session's own `call` reads an
  // options argument only when one was passed.
  const wrapped = async (...args) => {
    let reply;
    try {
      reply = await call(...args);
    } catch (thrown) {
      throw normalizeError(thrown);
    }
    if (reply && typeof reply === "object" && reply.ok === false) throw normalizeError(reply);
    return reply;
  };
  return {
    major,
    range,
    version,
    call: wrapped,
    capabilities: capabilitiesOf(greeting, version),
    events: eventsOf(greeting),
    parseResult,
    parseEvent,
  };
}
