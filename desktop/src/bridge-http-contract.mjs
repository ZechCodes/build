import {
  canonicalOrigin,
  transportError,
  MAX_JSON_BYTES,
} from "./bridge-http-network.mjs";

export const HOSTED_ORIGIN = "https://getbuild.ing";
export const fail = (code) => {
  throw transportError(code);
};
export function object(value, code = "invalid_params") {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(code);
  return value;
}
export function exact(value, keys, code = "invalid_params") {
  object(value, code);
  if (Object.keys(value).some((key) => !keys.includes(key))) fail(code);
  return value;
}
export function identifier(value, code = "invalid_params") {
  if (
    typeof value !== "string" ||
    !value.length ||
    value.length > 128 ||
    /[\x00-\x20\x7f]/u.test(value) ||
    value === "." ||
    value === ".."
  )
    fail(code);
  return value;
}
export const segment = (value) => encodeURIComponent(identifier(value));

export function verifiedMint(value, expected = {}, issuer = HOSTED_ORIGIN) {
  object(value, "invalid_response");
  for (const name of ["account_id", "device_id", "endpoint_epoch", "family_id"])
    identifier(value[name], "invalid_response");
  if (
    value.issuer !== issuer ||
    canonicalOrigin(value.origin) !== value.origin
  )
    fail("binding_mismatch");
  for (const [name, expectedValue] of Object.entries(expected))
    if (value[name] !== expectedValue) fail("binding_mismatch");
  validateCredential(value);
  return value;
}

function validateCredential(value) {
  if (
    typeof value.access_token !== "string" ||
    !/^[\x21-\x7e]{16,4096}$/u.test(value.access_token)
  )
    fail("invalid_response");
  if (
    !Number.isFinite(value.expires_at) ||
    value.expires_at * 1000 <= Date.now() ||
    value.expires_at * 1000 > Date.now() + 300000
  )
    fail("invalid_response");
  if (
    value.scope !== "bridge:read bridge:write" &&
    value.scope !== "bridge:read"
  )
    fail("invalid_response");
}

function headerIdentifier(value) {
  if (typeof value !== "string" || !/^[\x21-\x7e]{1,128}$/u.test(value))
    fail("invalid_options");
  return value;
}
function operationId(value) {
  if (
    typeof value !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u.test(
      value,
    )
  )
    fail("invalid_options");
  return value;
}
function dependencyHeader(after) {
  if (!Array.isArray(after) || after.length > 8) fail("invalid_options");
  after.forEach(operationId);
  return JSON.stringify(after);
}

// Local subscription correlation only; this is not authority or a durable ID.
export function subscriptionRequest(options) {
  exact(options, ["requestId"], "invalid_options");
  return operationId(options.requestId);
}

export function requestOptions(options = {}) {
  exact(options, ["requestId", "priority", "write"], "invalid_options");
  if (
    options.priority !== undefined &&
    !["foreground", "background"].includes(options.priority)
  )
    fail("invalid_options");
  const headers = {
    "X-Build-Priority":
      options.priority === "background" ? "background" : "user",
  };
  if (options.requestId !== undefined)
    headers["X-Build-Request-Id"] = identifier(
      headerIdentifier(options.requestId),
      "invalid_options",
    );
  if (options.write !== undefined) {
    exact(options.write, ["operation_id", "after"], "invalid_options");
    headers["Idempotency-Key"] = operationId(options.write.operation_id);
    if (options.write.after !== undefined) {
      headers["X-Build-After"] = dependencyHeader(options.write.after);
    }
  }
  return headers;
}

function validateRoute(route) {
  if (
    !["GET", "POST", "PUT", "PATCH", "DELETE"].includes(route.verb) ||
    !route.path.startsWith("/v1/") ||
    route.path.includes("?")
  )
    fail("invalid_manifest");
  if (
    !Number.isSafeInteger(route.responseMaxBytes) ||
    route.responseMaxBytes <= 0 ||
    route.responseMaxBytes > 8 * MAX_JSON_BYTES
  )
    fail("invalid_manifest");
  validateInputLimit(route);
  validatePlacement(route);
}
function validateInputLimit(route) {
  const input = route.requestMaxBytes ?? MAX_JSON_BYTES;
  if (!Number.isSafeInteger(input) || input <= 0 || input > 8 * MAX_JSON_BYTES)
    fail("invalid_manifest");
}
function validatePlacement(route) {
  const query = route.queryParams ?? [];
  if (!Array.isArray(query) || query.length) fail("invalid_manifest");
  if (
    !Array.isArray(route.pathParams) ||
    !["none", "remaining"].includes(route.body)
  )
    fail("invalid_manifest");
}
function parameterSegment(value) {
  if (
    typeof value !== "string" ||
    !value.length ||
    value.length > 4096 ||
    /[\x00-\x1f\x7f]/u.test(value) ||
    value === "." ||
    value === ".."
  )
    fail("invalid_params");
  return encodeURIComponent(value);
}
function placedBody(route, body) {
  if (route.body === "none" || route.verb === "GET") {
    if (Object.keys(body).length) fail("invalid_params");
    return undefined;
  }
  return body;
}
export function buildRoute(manifest, operationId, params) {
  object(params);
  const route = manifest?.operations?.find(
    (value) => value.operationId === operationId,
  );
  if (!route) fail("unsupported_operation");
  validateRoute(route);
  let path = route.path;
  const body = Object.fromEntries(
    Object.entries(params).filter(([, value]) => value !== undefined),
  );
  for (const key of route.pathParams) {
    path = path.replaceAll(`{${key}}`, parameterSegment(params[key]));
    delete body[key];
  }
  if (
    /[{}]/u.test(path) ||
    new URL(path, HOSTED_ORIGIN).origin !== HOSTED_ORIGIN
  )
    fail("invalid_manifest");
  return {
    path,
    method: route.verb,
    body: placedBody(route, body),
    maxBytes: route.responseMaxBytes,
    maxBody: route.requestMaxBytes ?? MAX_JSON_BYTES,
    requiresEvents: route.requires_events === true,
  };
}

export function compatibleTransport(manifest, transport) {
  if (
    manifest?.version !== 1 ||
    transport?.resource_manifest_version !== 1 ||
    transport.terminal !== "webrtc" ||
    transport.durable_events !== false ||
    !Array.isArray(transport.operation_ids)
  )
    fail("incompatible_transport");
  const supported = new Set(transport.operation_ids);
  if (manifest.operations.some((route) => !supported.has(route.operationId)))
    fail("incompatible_transport");
}

// These handles belong to a native Opening or an IPC attempt. Wire request IDs
// use monotonically increasing decimal strings within each IPC terminal.
const terminalHandle = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

function checkTerminalHandle(handle) {
  if (typeof handle !== "string" || !terminalHandle.test(handle)) fail("invalid_params");
}

export function deliverySequence(sequence) {
  if (!Number.isSafeInteger(sequence) || sequence < 1) fail("invalid_params");
  return sequence;
}

export function terminalRequestSequence(id) {
  if (typeof id !== "string" || !/^[1-9][0-9]{0,15}$/u.test(id)) fail("invalid_params");
  return deliverySequence(Number(id));
}

function terminalObject(value) {
  object(value);
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) fail("invalid_params");
  return value;
}

const methodsWithTermId = new Set(["term.attach", "term.input", "term.resize", "term.ack", "term.close"]);
const stringTerminalParams = new Set([
  "workspace_id", "run_id", "project_id", "worktree_id", "source_id", "source_path",
  "kind", "id", "agent_id", "conversation_id",
]);

function terminalCallParams(method, params) {
  identifier(method);
  terminalObject(params);
  if (methodsWithTermId.has(method)) {
    const id = params.term_id;
    if (typeof id !== "string" || !id.length || id.length > 4096) fail("invalid_params");
  }
  for (const [key, value] of Object.entries(params))
    if (stringTerminalParams.has(key) && value != null && typeof value !== "string") fail("invalid_params");
}

export function terminalIpcArguments(method, args) {
  checkTerminalHandle(args[0]);
  switch (method) {
    case "terminalOpen": {
      checkTerminalHandle(args[1]);
      const body = exact(terminalObject(args[2]), ["opening_generation", "retry"]);
      identifier(body.opening_generation);
      if (Object.hasOwn(body, "retry") && body.retry !== true) fail("invalid_params");
      break;
    }
    case "terminalCall":
      terminalCallParams(args[1], args[2]);
      terminalRequestSequence(args[3]);
      break;
    case "terminalConsume":
      deliverySequence(args[1]);
      break;
    case "terminalDetach":
      terminalRequestSequence(args[1]);
      break;
  }
}
