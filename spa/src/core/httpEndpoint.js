// A stored URL is only a hint. Hosted discovery and every renewal establish
// authority separately; this function does no DNS lookup or network request.
export function canonicalBridgeOrigin(input) {
  if (typeof input !== "string" || input.length > 2048) throw invalidOrigin();
  const raw = input.trim();
  const match = /^https:\/\/([^/?#]+)\/?$/i.exec(raw);
  if (!match || /[\\@%\s]/u.test(match[1])) throw invalidOrigin();
  let parsed;
  try { parsed = new URL(raw); } catch { throw invalidOrigin(); }
  if (parsed.hostname.endsWith(".") || parsed.port === "0") throw invalidOrigin();
  // WHATWG accepts octal/hexadecimal and shortened IPv4; never silently change
  // what the user entered into a different network authority.
  if (/^\d+\.\d+\.\d+\.\d+$/.test(parsed.hostname)) {
    const enteredHost = match[1].split(":")[0];
    if (enteredHost !== parsed.hostname) throw invalidOrigin();
  }
  return parsed.origin;
}

const invalidOrigin = () => Object.assign(new Error("Enter an HTTPS origin with no credentials, path, query or fragment."), {
  code: "invalid_origin", error_code: "invalid_origin", uncertain: false,
});

export function httpFailure(code, uncertain = false) {
  const messages = {
    network: "Could not reach the HTTPS bridge. Check its certificate trust, network access, CORS and browser local-network permission.",
    authentication: "Bridge authorization expired or was refused. Verify this device with your account again.",
    protocol: "The bridge returned an invalid HTTP response.",
    scope_ended: "This bridge connection has ended.",
    stream_reset: "Live updates ended. Reconnecting and refreshing current state…",
    unsupported: "This bridge does not provide all required HTTP operations. Use WebRTC or update the bridge.",
    timeout: "The HTTPS bridge did not answer before the deadline.",
    network_error: "Could not reach the bridge or verify its HTTPS connection.",
    unauthorized: "Bridge account authorization was refused. Verify this device again.",
    binding_mismatch: "The HTTPS device or endpoint binding changed. Verify its address again.",
    stale_owner: "This bridge connection belongs to a previous account session. Verify its address again.",
    untrusted_caller: "This app cannot use the native bridge connection. Reload and sign in again.",
    invalid_handle: "This bridge connection has ended. Verify the address again.",
    incompatible_transport: "This bridge does not support this app's required HTTPS operations.",
    unsupported_operation: "This bridge does not support the requested HTTPS operation.",
    invalid_manifest: "This app's HTTPS operation manifest is invalid. Reload the app.",
    invalid_origin: "Enter an HTTPS origin with no credentials, path, query or fragment.",
    busy: "The bridge is busy. Try again after its current requests finish.",
    canceled: "The bridge request was cancelled.",
    request_too_large: "The bridge request exceeds this operation's size limit.",
    response_too_large: "The bridge response exceeds this operation's size limit.",
  };
  return Object.assign(new Error(messages[code] || "The HTTPS bridge request failed."), {
    code, error_code: code, uncertain,
  });
}
