// The notification key (#200, planning/v2/Push Content Security Checklist.md).
//
// One ECDH P-256 key pair per push subscription, generated here by WebCrypto
// with the private half non-extractable. The pair lives in IndexedDB (database
// `build-push`, store `keys`, keyed by the subscription id) where the service
// worker (public/sw.js) reads it to open sealed pushes. The public half goes to
// bridges only over the E2EE session (core/pushKeySync.js); the api never sees
// it, which is the only thing stopping anyone else from sealing a push that
// opens.
//
// The service worker cannot import this module, so it writes the database
// layout and the subscription id out again; test/pushSealed.test.js holds the
// two copies together.

const PUSH_DB_NAME = "build-push";
const PUSH_DB_VERSION = 1;
const KEYS_STORE = "keys";
const SEEN_STORE = "seen";

/** Unpadded base64url. */
export function b64u(bytes) {
  let binary = "";
  for (const byte of new Uint8Array(bytes)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** sid = b64u(SHA-256(push endpoint)): the name the api and bridges know a
 *  subscription by, 43 characters. */
export async function subscriptionIdOf(endpoint) {
  return b64u(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(endpoint)));
}

function upgradePushDb(db) {
  if (!db.objectStoreNames.contains(KEYS_STORE)) db.createObjectStore(KEYS_STORE, { keyPath: "sid" });
  if (!db.objectStoreNames.contains(SEEN_STORE)) db.createObjectStore(SEEN_STORE, { keyPath: "nonce" });
}

function openPushDb() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(PUSH_DB_NAME, PUSH_DB_VERSION);
    request.onupgradeneeded = () => upgradePushDb(request.result);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** Run `work(store)` in one transaction on the keys store and answer what the
 *  request it returns (if any) produced, once the transaction commits. */
async function withKeys(mode, work) {
  const db = await openPushDb();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(KEYS_STORE, mode);
      const request = work(tx.objectStore(KEYS_STORE));
      tx.oncomplete = () => resolve(request ? request.result : undefined);
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

/** The stored key record for a subscription id, or null. */
export async function readPushKey(sid) {
  return (await withKeys("readonly", (store) => store.get(sid))) || null;
}

/** Every sid a key is stored under. */
export async function storedKeyIds() {
  return (await withKeys("readonly", (store) => store.getAllKeys())) || [];
}

/** Forget every key but the one for `keepSid` (null forgets them all). */
export async function deletePushKeysExcept(keepSid = null) {
  const sids = await storedKeyIds();
  await withKeys("readwrite", (store) => {
    for (const sid of sids) if (sid !== keepSid) store.delete(sid);
    return null;
  });
}

async function generateKeyRecord(sid) {
  const pair = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
  // A public key is always exportable; only the private half is held back.
  const publicKey = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  return { sid, privateKey: pair.privateKey, publicKey };
}

/** Forget one subscription's key. */
export async function deletePushKey(sid) {
  await withKeys("readwrite", (store) => {
    store.delete(sid);
    return null;
  });
}

let keyChain = Promise.resolve();

/**
 * The key for this subscription, generated (and every other sid's key deleted)
 * when there is none — a new subscription is a new sid and a new key. Answers
 * `{ sid, publicKey, created }`, `publicKey` as b64u of the 65-byte point.
 */
export function ensurePushKey(sid) {
  // One at a time: greetings from several bridges land together, and two
  // generations for one new sid would leave the bridges holding different keys.
  const next = keyChain.then(() => ensureNow(sid));
  keyChain = next.catch(() => {});
  return next;
}

async function ensureNow(sid) {
  const held = await readPushKey(sid);
  if (held) return { sid, publicKey: b64u(held.publicKey), created: false };
  const record = await generateKeyRecord(sid);
  await withKeys("readwrite", (store) => store.put(record));
  await deletePushKeysExcept(sid);
  return { sid, publicKey: b64u(record.publicKey), created: true };
}
