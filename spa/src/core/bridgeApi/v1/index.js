// The v1 adapter: everything this SPA knows about API major 1.
//
// A surface never asks what version the bridge reports — it asks the adapter's
// `capabilities`, which are derived once from the greeting and the minor
// version and never from probing a method to see whether it is refused. That
// is the whole point of the version. The floor is 1.2: the client reads what
// a push carries and polls nothing, and a bridge below 1.2 pushes keys, not
// bodies, so it is gated rather than served a client that would never move.
//
// The adapter also owns error normalisation. From 1.1 a refusal carries
// `error_code`, `retryable` and `details` beside the string; from 1.0 it is
// the string alone, which becomes `ApiError("unknown")` with the text intact.
// A view therefore reads `error.code` whatever it is talking to.

/** The range of bridge versions this adapter claims. */
export const range = ">=1.2.0 <2.0.0";

/** The API major it is the adapter for. */
export const major = 1;

/** `ApiError.code` for a refusal that named none — a 1.0 bridge's string. */
export const UNKNOWN_CODE = "unknown";

/** The closed set of codes a 1.x bridge refuses with (`api/mod.rs`), plus the
 *  client-side stand-in for a refusal that named none. Additive in a minor:
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
 *  a throw — a later minor may add one. `fixtures/api/v1/events.json` carries
 *  one example of each, and both ends are held to it. */
export const EVENT_TYPES = Object.freeze([
  "board.changed",
  "entity.changed",
  "changes",
  "term.output",
  "term.reset",
  "term.closed",
  "rtc.ice",
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

/** The greeting of a bridge that named a version, with its optional sections
 *  filled in — anything else is pre-alpha and claims nothing. */
function statedGreeting(greeting) {
  const version = greeting && greeting.api_version;
  if (typeof version !== "string" || !version) return null;
  return {
    changes: greeting.changes || {},
    requests: greeting.requests || {},
    errors: greeting.errors || {},
    issues: greeting.issues || {},
  };
}

/**
 * The subscription kinds a bridge says it carries.
 *
 * A greeting that names none is read as carrying none, and a caller asking
 * "does this bridge carry X" gets no for every X. That is the safe direction:
 * every kind in one `changes.subscribe` shares that call's fate, so asking for
 * one the bridge does not know risks the whole subscription — and with it the
 * kinds that would have worked. A caller may still ask for a kind unguarded;
 * this is for the ones worth checking first.
 */
const kindsOf = (stated) =>
  Array.isArray(stated.changes.kinds) ? stated.changes.kinds.filter((kind) => typeof kind === "string") : [];

/** A boolean the greeting may state outright; otherwise the minor decides. */
function capability(stated, minorFloor, minor) {
  if (typeof stated === "boolean") return stated;
  return minor >= minorFloor;
}

function minorOf(version) {
  const parts = String(version || "").split(".");
  return Number(parts[1]) || 0;
}

/**
 * What this bridge can do, from the greeting and the minor version only.
 * A pre-alpha bridge (no `api_version`, read as `0.0.0`) gets every flag off
 * whatever else it claims: nothing before 1.0 is a contract.
 */
export function capabilitiesOf(greeting, version) {
  const stated = statedGreeting(greeting);
  if (!stated)
    return {
      changes: { subscriptions: false, kinds: [] },
      requests: { priority: false },
      errors: { codes: false },
      diffs: { perFile: false },
      issues: { attachments: false, watching: false },
    };
  const minor = minorOf(version);
  return {
    changes: { subscriptions: capability(stated.changes.subscriptions, 1, minor), kinds: kindsOf(stated) },
    requests: { priority: capability(stated.requests.priority, 1, minor) },
    errors: { codes: capability(stated.errors.codes, 1, minor) },
    // `git.changeset_diff`, and the per-file counts and keys a stack drawn
    // without hunks needs (1.4). The greeting states nothing about it, so the
    // minor is the whole of the answer — and a surface that asked a 1.3 bridge
    // for hunks per file would draw a stack of files that never load.
    diffs: { perFile: minor >= 4 },
    // Files on an issue (1.8). A bridge that has them says so outright; the
    // minor answers for one that predates the flag but not the verbs.
    issues: {
      attachments: capability(stated.issues.attachments, 8, minor),
      // Watching, and the read marks that go with it (1.9). Stated outright for
      // the same reason attachments are: a switch wired to a verb the bridge has
      // never heard of can only refuse, and the read mark would produce one
      // refusal per glance at an issue.
      //
      // It sits in the issues group and covers `conversation.watch` too, which
      // is a naming stretch — the two verbs ship together in 1.9 and are one
      // capability, so one flag answers for both.
      watching: capability(stated.issues?.watching, 9, minor),
    },
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
  const version = String(greeting?.api_version || "0.0.0");
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
