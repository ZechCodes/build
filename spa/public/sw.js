// Build push worker — deliberately tiny and cache-free, and self-contained: a
// service worker cannot import the app's modules, so the few helpers it shares
// with src/pushKeys.js (the IndexedDB layout, the subscription id) are written
// out again here and held together by test/pushSealed.test.js.
//
// E2EE invariant (#200, planning/v2/Push Content Security Checklist.md): a push
// says what happened, and only this browser can read it. The api wraps every
// push as {task_id, kind, url, sealed}. `task_id`, `kind` and `url` are opaque
// cleartext metadata; `sealed` is a blob a bridge sealed to THIS subscription's
// notification key. The worker opens it with the non-extractable private key
// the app keeps in IndexedDB and shows the title and body inside. The api, the
// push service and every log see only the subscription id and ciphertext.
//
// The sealing is ECIES with an ephemeral sender key, so it is NOT
// sender-authenticated: anyone holding this subscription's notification public
// key can seal a blob that opens. Forgery is prevented only because that public
// key never leaves the E2EE path: the browser generates it and hands it only to
// bridges over the E2EE session (`push.registerKey`); the api never receives
// it. As defence in depth a sealed url is used only if it starts with /app/#/,
// so even a forged blob cannot navigate outside the app.
//
// Anything that does not open cleanly shows the #191 generic copy for the
// kind: no subscription, no key, a bad tag, a stale or future `iat`, a nonce
// already seen, malformed plaintext, or a payload with no `sealed` at all (an
// old bridge). Failures log a reason code at debug level and never content.
// A push fires only for what adds to the unread counter (#191): an agent's
// conversation (`agent`) or a watched task (`task`).
//
// There is NO fetch handler on purpose: an E2EE app must never be served from a
// stale cache.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

// Kind → the generic line shown on the notification. Unknown kinds fall back to
// the agent copy, so a new bridge kind never renders blank.
const KIND_BODY = {
  agent: "An agent needs you",
  task: "New activity on a task",
};

const NOTIFICATION_ICON = "/app/static/icon-192.png";

// The TTL the api sets on every push: skriftapp/buildapp/web_push.py
// PUSH_TTL_SECONDS. test/pushSealed.test.js holds the two equal.
const PUSH_TTL_SECONDS = 0;
const CLOCK_SKEW_SECONDS = 300;
// Nonces are remembered for as long as a blob carrying one could still pass
// the freshness check: its iat may be up to one skew ahead of the clock that
// saw it, and it stays acceptable one TTL and one skew after its iat.
const SEEN_WINDOW_MS = (PUSH_TTL_SECONDS + 2 * CLOCK_SKEW_SECONDS) * 1000;
const SEEN_CAP = 256;

const SEAL_INFO = "build-push-v1";
const SEAL_VERSION = 0x01;
const POINT_BYTES = 65;
const NONCE_BYTES = 12;
const TAG_BYTES = 16;
const APP_LINK_PREFIX = "/app/#/";
const OPEN_MESSAGE = "build.push.open";

// The IndexedDB the app writes the notification key into (src/pushKeys.js).
const PUSH_DB_NAME = "build-push";
const PUSH_DB_VERSION = 1;
const KEYS_STORE = "keys";
const SEEN_STORE = "seen";

function bodyForKind(kind) {
  return KIND_BODY[kind] || KIND_BODY.agent;
}

// ---- encoding ------------------------------------------------------------------

function b64uEncode(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64uDecode(text) {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) throw new SealFailure("malformed");
  const padded = text.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (text.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function concatBytes(...parts) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

const utf8 = (text) => new TextEncoder().encode(text);

/** sid = b64u(SHA-256(push endpoint)), the only name the api and bridges use. */
async function subscriptionId(endpoint) {
  return b64uEncode(new Uint8Array(await self.crypto.subtle.digest("SHA-256", utf8(endpoint))));
}

/** A failure with a reason code — the only thing ever logged about it. */
class SealFailure extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

// ---- IndexedDB -----------------------------------------------------------------

function upgradePushDb(db) {
  if (!db.objectStoreNames.contains(KEYS_STORE)) db.createObjectStore(KEYS_STORE, { keyPath: "sid" });
  if (!db.objectStoreNames.contains(SEEN_STORE)) db.createObjectStore(SEEN_STORE, { keyPath: "nonce" });
}

function openPushDb() {
  return new Promise((resolve, reject) => {
    const request = self.indexedDB.open(PUSH_DB_NAME, PUSH_DB_VERSION);
    request.onupgradeneeded = () => upgradePushDb(request.result);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function readKey(db, sid) {
  return new Promise((resolve, reject) => {
    const request = db.transaction(KEYS_STORE, "readonly").objectStore(KEYS_STORE).get(sid);
    request.onsuccess = () => resolve(request.result || null);
    request.onerror = () => reject(request.error);
  });
}

/** Drop what is older than the window, then the oldest past the cap. */
function pruneSeen(store, rows, nowMs) {
  const kept = [];
  for (const row of rows) {
    if (nowMs - row.seenAt > SEEN_WINDOW_MS) store.delete(row.nonce);
    else kept.push(row);
  }
  kept.sort((a, b) => a.seenAt - b.seenAt);
  while (kept.length > SEEN_CAP) store.delete(kept.shift().nonce);
  return kept;
}

/** Record a nonce in one transaction; false if it was already seen. No
 *  ordering check: several bridges with skewed clocks share one sid. */
function admitNonce(db, nonce, nowMs) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(SEEN_STORE, "readwrite");
    const store = tx.objectStore(SEEN_STORE);
    let admitted = false;
    const all = store.getAll();
    all.onsuccess = () => {
      if (all.result.some((row) => row.nonce === nonce)) return;
      admitted = true;
      pruneSeen(store, [...all.result, { nonce, seenAt: nowMs }], nowMs);
      store.put({ nonce, seenAt: nowMs });
    };
    tx.oncomplete = () => resolve(admitted);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

// ---- opening a sealed blob -------------------------------------------------------

function splitBlob(bytes) {
  if (bytes.length < 1 + POINT_BYTES + NONCE_BYTES + TAG_BYTES || bytes[0] !== SEAL_VERSION) {
    throw new SealFailure("malformed");
  }
  return {
    epk: bytes.slice(1, 1 + POINT_BYTES),
    nonce: bytes.slice(1 + POINT_BYTES, 1 + POINT_BYTES + NONCE_BYTES),
    ciphertext: bytes.slice(1 + POINT_BYTES + NONCE_BYTES),
  };
}

const aadFor = (sid, kind, entityId) => utf8(`${SEAL_INFO}\0${sid}\0${kind}\0${entityId}`);

async function decryptBlob(key, blob, aad) {
  const subtle = self.crypto.subtle;
  const ecdh = { name: "ECDH", namedCurve: "P-256" };
  const ephemeral = await subtle.importKey("raw", blob.epk, ecdh, false, []);
  const shared = await subtle.deriveBits({ name: "ECDH", public: ephemeral }, key.privateKey, 256);
  const ikm = await subtle.importKey("raw", shared, "HKDF", false, ["deriveKey"]);
  const info = concatBytes(utf8(SEAL_INFO), blob.epk, new Uint8Array(key.publicKey));
  const aes = await subtle.deriveKey(
    { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0), info },
    ikm,
    { name: "AES-GCM", length: 256 },
    false,
    ["decrypt"],
  );
  try {
    const params = { name: "AES-GCM", iv: blob.nonce, additionalData: aad, tagLength: TAG_BYTES * 8 };
    return new Uint8Array(await subtle.decrypt(params, aes, blob.ciphertext));
  } catch {
    throw new SealFailure("tag");
  }
}

function parseMessage(plaintext) {
  let message;
  try {
    message = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plaintext));
  } catch {
    throw new SealFailure("malformed");
  }
  const valid = message && message.v === 1 && typeof message.title === "string"
    && typeof message.body === "string" && typeof message.iat === "number" && Number.isFinite(message.iat);
  if (!valid) throw new SealFailure("malformed");
  return message;
}

function checkFresh(iat, nowMs) {
  const now = nowMs / 1000;
  if (iat < now - (PUSH_TTL_SECONDS + CLOCK_SKEW_SECONDS)) throw new SealFailure("stale");
  if (iat > now + CLOCK_SKEW_SECONDS) throw new SealFailure("future");
}

/** The cleartext the AAD binds: never trusted for anything but that. */
function sealedFields(payload) {
  const fields = payload && typeof payload.sealed === "string" ? payload : null;
  if (!fields) throw new SealFailure("unsealed");
  if (typeof fields.kind !== "string" || typeof fields.task_id !== "string") throw new SealFailure("malformed");
  return { blob: fields.sealed, kind: fields.kind, entityId: fields.task_id };
}

/** Open the payload's sealed blob with this browser's own key, or throw. The
 *  sid is computed from this worker's own subscription, never read off the
 *  payload. */
async function openSealed(payload) {
  const fields = sealedFields(payload);
  const subscription = await self.registration.pushManager.getSubscription();
  if (!subscription) throw new SealFailure("no-subscription");
  const sid = await subscriptionId(subscription.endpoint);
  const db = await openPushDb();
  try {
    const key = await readKey(db, sid);
    if (!key) throw new SealFailure("no-key");
    const blob = splitBlob(b64uDecode(fields.blob));
    const message = parseMessage(await decryptBlob(key, blob, aadFor(sid, fields.kind, fields.entityId)));
    checkFresh(message.iat, Date.now());
    if (!(await admitNonce(db, b64uEncode(blob.nonce), Date.now()))) throw new SealFailure("replay");
    return message;
  } finally {
    db.close();
  }
}

function fellBack(error) {
  console.debug("sw: push shown with the generic copy", error instanceof SealFailure ? error.code : "error");
  return null;
}

// ---- the notification ------------------------------------------------------------

function readPayload(event) {
  try {
    return event.data ? event.data.json() : null;
  } catch {
    return null; // Not JSON — render the generic notification anyway.
  }
}

// Only a same-origin absolute path is a safe deep link. Reject a
// protocol-relative "//host/path" (a buggy payload) — it is cross-origin and
// would navigate away from the app; fall back to the app root.
function cleartextUrl(payload) {
  const url = payload && payload.url;
  return typeof url === "string" && url.startsWith("/") && !url.startsWith("//") ? url : "/app/";
}

function cleartextOf(payload) {
  return {
    url: cleartextUrl(payload),
    kind: payload && typeof payload.kind === "string" ? payload.kind : "agent",
    taskId: payload && typeof payload.task_id === "string" ? payload.task_id : "",
  };
}

async function showPush(event) {
  const payload = readPayload(event);
  const meta = cleartextOf(payload);
  const opened = await openSealed(payload).catch(fellBack);
  const sealedUrl = opened && typeof opened.url === "string" && opened.url.startsWith(APP_LINK_PREFIX);
  // tag=task_id so repeated pushes for the same entity collapse into one
  // notification instead of stacking; a payload without an id falls back to a
  // single shared tag.
  const tag = meta.taskId ? `build-task-${meta.taskId}` : "build-attention";
  return self.registration.showNotification(opened ? opened.title : "Build", {
    body: opened ? opened.body : bodyForKind(meta.kind),
    icon: NOTIFICATION_ICON,
    tag,
    renotify: true,
    data: { url: sealedUrl ? opened.url : meta.url },
  });
}

self.addEventListener("push", (event) => {
  event.waitUntil(showPush(event));
});

/** Ask an open app window to route to the link itself (src/push.js), which
 *  opens the chat without a reload. False when the message cannot be sent. */
function tellApp(client, url) {
  try {
    client.postMessage({ type: OPEN_MESSAGE, url });
    return true;
  } catch {
    return false;
  }
}

async function navigateOrOpen(client, url) {
  if (!("navigate" in client)) return client;
  try {
    return await client.navigate(url);
  } catch (err) {
    // navigate() rejects for an uncontrolled window (shift-reload, mid-update)
    // or a malformed deep link. Don't swallow it silently: log it and open a
    // fresh window on the target so the click still lands there.
    console.warn("sw: deep-link navigate failed; opening a new window", err);
    return self.clients.openWindow(url);
  }
}

// A link inside the app is marked as a notification open (#200): the app lands
// that one open on the latest message, then drops the mark from the URL
// (src/core/router.js `takePushOpenMark`), so a reload lands on the unread line.
const PUSH_OPEN_MARK = "from=push";

function markedAsPushOpen(url) {
  if (!url.startsWith(APP_LINK_PREFIX)) return url;
  return `${url}${url.includes("?") ? "&" : "?"}${PUSH_OPEN_MARK}`;
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = markedAsPushOpen((event.notification.data && event.notification.data.url) || "/app/");
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });
      const existing = windows.find((w) => new URL(w.url).pathname.startsWith("/app"));
      // No window: a cold start, where the router reads the hash at boot.
      if (!existing) return self.clients.openWindow(url);
      // Focus the open app and steer it to the deep link so the click always
      // lands on the right place, not just whatever was last open.
      await existing.focus();
      if (tellApp(existing, url)) return existing;
      return navigateOrOpen(existing, url);
    })(),
  );
});
