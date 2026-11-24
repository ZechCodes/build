import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { exact, fail, identifier } from "./bridge-http-contract.mjs";
import { canonicalOrigin } from "./bridge-http-network.mjs";

const UUID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/iu;

function accountOf(access) {
  // Main only receives this token from the configured HTTPS token exchange.
  // Its account names a local preference file. Hosted verifies authorization.
  let claims;
  try { claims = JSON.parse(Buffer.from(access.split(".")[0], "base64url")); }
  catch { fail("sign_in"); }
  if (claims?.type !== "access" || claims.client_id !== "build-desktop"
    || !UUID.test(claims.user_id) || !(claims.exp > Date.now() / 1000)) fail("sign_in");
  return claims.user_id.toLowerCase();
}

function clean(setting, deviceId) {
  exact(setting, ["deviceId", "origin", "selected", "fingerprint"]);
  if (setting.deviceId !== deviceId || !["http", "webrtc"].includes(setting.selected)) fail("invalid_params");
  const fingerprint = setting.fingerprint === undefined ? undefined : identifier(setting.fingerprint);
  return { deviceId, origin: canonicalOrigin(setting.origin), selected: setting.selected, ...(fingerprint && { fingerprint }) };
}

function canLearnPin(current, expected, replacement) {
  if (!current || !replacement?.fingerprint || expected.fingerprint) return false;
  return current.origin === expected.origin && replacement.origin === expected.origin
    && current.selected === expected.selected && replacement.selected === expected.selected
    && (!current.fingerprint || current.fingerprint === replacement.fingerprint);
}

export function createBridgeHttpSettings(userData) {
  const directory = join(userData, "bridge-http-settings");
  const pathFor = (accountId) => join(directory, accountId + ".json");
  function read(access) {
    const accountId = accountOf(access);
    try {
      const stored = JSON.parse(readFileSync(pathFor(accountId), "utf8"));
      return { accountId, connections: stored.slice(0, 16).map((entry) => clean(entry, identifier(entry.deviceId))) };
    } catch (error) {
      if (error.code === "ENOENT") return null;
      throw error;
    }
  }
  function write(access, input) {
    exact(input, ["accountId", "deviceId", "setting", "expected"]);
    const accountId = accountOf(access);
    if (accountId !== input.accountId) fail("sign_in");
    const deviceId = identifier(input.deviceId);
    const saved = read(access)?.connections || [];
    const replacement = input.setting === null ? null : clean(input.setting, deviceId);
    if (input.expected !== undefined) {
      const current = saved.find((entry) => entry.deviceId === deviceId);
      if (!canLearnPin(current, clean(input.expected, deviceId), replacement)) return false;
    }
    const connections = saved.filter((entry) => entry.deviceId !== deviceId);
    if (replacement) connections.push(replacement);
    if (connections.length > 16) fail("busy");
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const staging = join(directory, randomUUID() + ".tmp");
    try {
      writeFileSync(staging, JSON.stringify(connections), { mode: 0o600, flag: "wx" });
      renameSync(staging, pathFor(accountId));
    } finally {
      rmSync(staging, { force: true });
    }
    return true;
  }
  return { read, write };
}
