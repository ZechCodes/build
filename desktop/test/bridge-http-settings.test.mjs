import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { createBridgeHttpSettings } from "../src/bridge-http-settings.mjs";

const account = "97b4e5c0-67fb-4e7e-a2bd-1d267d5bae38";
const other = "be97a3af-3302-404b-8b67-8eef817a2fe1";
const deviceId = "3b535cde-bd66-4bd5-b11b-be5f19ebbf94";
const access = (user_id) => Buffer.from(JSON.stringify({ user_id, type: "access", client_id: "build-desktop", exp: Date.now() / 1000 + 900 })).toString("base64url") + ".opaque";

test("desktop settings survive a fresh process and expose only the signed-in account's choices", (t) => {
  const userData = mkdtempSync(join(tmpdir(), "build-http-settings-"));
  t.after(() => rmSync(userData, { recursive: true, force: true }));
  const setting = { deviceId, origin: "https://127.0.0.1:8443", selected: "http" };
  const store = createBridgeHttpSettings(userData);
  store.write(access(account), { accountId: account, deviceId, setting });
  const reopened = createBridgeHttpSettings(userData);
  assert.deepEqual(reopened.read(access(account)), { accountId: account, connections: [setting] });
  assert.equal(reopened.read(access(other)), null);
  assert.throws(() => reopened.read(null), { code: "sign_in" });
  assert.throws(() => reopened.write(access(account), { accountId: other, deviceId, setting }), { code: "sign_in" });
  assert.throws(() => reopened.write(access(account), { accountId: account, deviceId, setting: { ...setting, familyId: "not-a-setting" } }), { code: "invalid_params" });
  reopened.write(access(account), { accountId: account, deviceId, setting: null });
  assert.deepEqual(reopened.read(access(account)).connections, []);
});

test("a learned pin updates only the exact legacy setting and survives restart", (t) => {
  const userData = mkdtempSync(join(tmpdir(), "build-http-pin-"));
  t.after(() => rmSync(userData, { recursive: true, force: true }));
  const token = access(account);
  const store = createBridgeHttpSettings(userData);
  const setting = { deviceId, origin: "https://127.0.0.1:8443", selected: "http" };
  store.write(token, { accountId: account, deviceId, setting });
  const pinned = { ...setting, fingerprint: "paired-fingerprint" };
  assert.equal(store.write(token, { accountId: account, deviceId, setting: pinned, expected: setting }), true);
  const reopened = createBridgeHttpSettings(userData);
  assert.deepEqual(reopened.read(token).connections, [pinned]);
  assert.equal(reopened.write(token, { accountId: account, deviceId, setting: { ...pinned, fingerprint: "changed" }, expected: setting }), false);
  assert.deepEqual(reopened.read(token).connections, [pinned]);
});
