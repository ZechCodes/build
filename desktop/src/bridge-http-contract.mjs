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

export function verifiedMint(value, expected = {}) {
  object(value, "invalid_response");
  for (const name of ["account_id", "device_id", "endpoint_epoch", "family_id"])
    identifier(value[name], "invalid_response");
  if (
    value.issuer !== HOSTED_ORIGIN ||
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
