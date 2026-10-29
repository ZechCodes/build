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
  };
  return Object.assign(new Error(messages[code] || "The HTTPS bridge request failed."), {
    code, error_code: code, uncertain,
  });
}
