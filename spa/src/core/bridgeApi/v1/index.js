// The adapter for API major 2: everything this SPA knows about the wire. The
// module keeps its v1 name — 2.0.0 broke one thing, the task rename (#190):
// every tracker and plan verb and feature name moved to `tasks.*` or
// `task.*`, so a 1.x bridge is gated as behind rather than served names it
// does not know.
//
// A surface never asks what version the bridge reports — it asks the adapter's
// `capabilities`, the names in the greeting. Every 2.x bridge sends the list;
// the minor table that stood in for it on bridges before 1.22 went with 1.x.
//
// The adapter also owns error normalisation. A refusal carries `error_code`,
// `retryable` and `details` beside the string; one that is the string alone
// becomes `ApiError("unknown")` with the text intact. A view therefore reads
// `error.code` whatever it is talking to.

import { satisfies } from "../semver.js";

/** The range of bridge versions this adapter claims. */
export const range = ">=2.0.0 <3.0.0";

/** The API major it is the adapter for. */
export const major = 2;

/** `ApiError.code` for a refusal that named none. */
export const UNKNOWN_CODE = "unknown";

/** The closed set of codes a bridge refuses with (`api/mod.rs`), plus the
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

/** Every push a bridge sends on a session: the change events, the terminal
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
  "models.changed",
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

/** The names a greeting announces. An absent or malformed list, or a
 *  greeting outside this major, claims nothing. */
function namesOf(greeting, version) {
  if (typeof greeting?.api_version !== "string" || !satisfies(version, range)) return new Set();
  return new Set(Array.isArray(greeting.capabilities) ? greeting.capabilities : []);
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
    bodies: { pages: names.has("bodies.pages"), mediaRawPages: names.has("fs.mediaRawPages") },
    tasks: {
      context: names.has("tasks.context"),
      attachments: names.has("tasks.attachments"),
      watching: names.has("tasks.watching"),
      doneSinceLeft: names.has("tasks.doneSinceLeft"),
      commentUserNotifies: names.has("tasks.commentUserNotifies"),
      listPaged: names.has("tasks.listPaged"),
    },
    conversations: { settings: names.has("conversations.settings") },
    github: { repos: names.has("github.repos") },
    messages: { context: names.has("messages.context") },
    threads: { postOperations: names.has("threads.postOperations"), attachmentChunks: names.has("thread.attachmentChunks") },
    branches: { finishDelete: names.has("branches.finishDelete") },
    push: { registerKey: names.has("push.registerKey"), revokeKey: names.has("push.revokeKey") },
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
