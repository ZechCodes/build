import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createTokenStore } from "../src/token-store.mjs";

// Deliberately recognizable ciphertext: no OS keychain or real credentials.
export function storageStub(overrides = {}) {
  return {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "gnome_libsecret",
    encryptString: (token) => Buffer.from(`encrypted:${token}`),
    decryptString: (bytes) => {
      const value = bytes.toString();
      if (!value.startsWith("encrypted:")) throw new Error("corrupt fixture");
      return value.slice(10);
    },
    ...overrides,
  };
}

function fixture(t, overrides) {
  const directory = mkdtempSync(join(tmpdir(), "desktop-token-store-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, "refresh-token.enc");
  const store = createTokenStore({ userData: directory, safeStorage: storageStub(overrides) });
  return { directory, path, store };
}

test("encrypted refresh token survives a new store and rotation replaces it with mode 0600", (t) => {
  const { directory, path, store } = fixture(t);
  assert.equal(store.load(), null);
  store.save("fixture-refresh-old-long");
  assert.equal(readFileSync(path, "utf8"), "encrypted:fixture-refresh-old-long");
  if (process.platform !== "win32") assert.equal(statSync(path).mode & 0o777, 0o600);
  chmodSync(path, 0o644);
  store.save("new");
  assert.equal(readFileSync(path, "utf8"), "encrypted:new");
  if (process.platform !== "win32") assert.equal(statSync(path).mode & 0o777, 0o600);
  assert.equal(createTokenStore({ userData: directory, safeStorage: storageStub() }).load(), "new");
  assert.deepEqual(readdirSync(directory), ["refresh-token.enc"]);
  store.clear();
  store.clear();
  assert.equal(existsSync(path), false);
});

for (const [name, overrides] of [
  ["unavailable encryption", { isEncryptionAvailable: () => false }],
  ["Linux basic_text backend", { getSelectedStorageBackend: () => "basic_text" }],
]) {
  test(`${name} never encrypts, decrypts, or writes a token`, (t) => {
    const { directory, path, store } = fixture(t, {
      ...overrides,
      encryptString: () => assert.fail("must not encrypt"),
      decryptString: () => assert.fail("must not decrypt"),
    });
    store.save("fixture-refresh");
    assert.deepEqual(readdirSync(directory), []);
    writeFileSync(path, "encrypted:previous");
    assert.equal(store.load(), null);
    store.clear();
    assert.equal(existsSync(path), false);
  });
}

test("corrupt ciphertext is discarded without returning credentials", (t) => {
  const { path, store } = fixture(t);
  writeFileSync(path, "corrupt fixture");
  assert.equal(store.load(), null);
  assert.equal(existsSync(path), false);
});

test("failed encryption leaves no plaintext or partial rotation on disk", (t) => {
  const { directory, path, store } = fixture(t, {
    encryptString: () => { throw new Error("fixture encryption failure"); },
  });
  writeFileSync(path, "encrypted:previous", { mode: 0o600 });
  assert.throws(() => store.save("fixture-secret"), /encryption failure/);
  assert.equal(readFileSync(path, "utf8"), "encrypted:previous");
  assert.deepEqual(readdirSync(directory), ["refresh-token.enc"]);
});

test("rotation replaces a symlink without writing to its target", (t) => {
  const { directory, path, store } = fixture(t);
  const target = join(directory, "unrelated-file");
  writeFileSync(target, "unrelated");
  symlinkSync(target, path);
  store.save("fixture-refresh");
  assert.equal(readFileSync(target, "utf8"), "unrelated");
  assert.equal(statSync(path).isFile(), true);
  assert.equal(store.load(), "fixture-refresh");
});
