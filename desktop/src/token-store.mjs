import { randomUUID } from "node:crypto";
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Main-process only. Never use Electron's unprotected Linux fallback.
export function createTokenStore({ userData, safeStorage }) {
  const path = join(userData, "refresh-token.enc");
  const encryptionAvailable = () => safeStorage.isEncryptionAvailable()
    && safeStorage.getSelectedStorageBackend?.() !== "basic_text";
  const clear = () => rmSync(path, { force: true });

  function load() {
    if (!encryptionAvailable()) return null;
    let encrypted;
    try {
      encrypted = readFileSync(path);
    } catch (error) {
      if (error.code === "ENOENT") return null;
      throw error;
    }
    try {
      return safeStorage.decryptString(encrypted) || null;
    } catch {
      clear();
      return null;
    }
  }

  function save(token) {
    if (!encryptionAvailable()) return;
    const encrypted = safeStorage.encryptString(token);
    mkdirSync(userData, { recursive: true, mode: 0o700 });
    const temporary = `${path}.${randomUUID()}.tmp`;
    try {
      const descriptor = openSync(temporary, "wx", 0o600);
      try {
        writeFileSync(descriptor, encrypted);
        fsyncSync(descriptor);
      } finally {
        closeSync(descriptor);
      }
      // Replace atomically: no append, torn ciphertext, or inherited permissions.
      renameSync(temporary, path);
    } finally {
      rmSync(temporary, { force: true });
    }
  }

  return { load, save, clear };
}
