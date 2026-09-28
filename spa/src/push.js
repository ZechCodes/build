// Web-push subscription management. The api only ever holds the subscription
// and relays ciphertext: what a push says is sealed by the bridge to this
// browser's notification key (#200), which is made here after subscribing and
// handed to the bridges over E2EE (core/pushKeySync.js).
//
// The service worker (public/sw.js, served same-origin at /app/sw.js) opens a
// sealed push, falls back to generic copy for anything else, and never caches
// anything. A click on its notification comes back here as `build.push.open`.

import {
  fetchVapidPublicKey,
  subscribePush,
  unsubscribePush,
} from "./api.js";
import { armNotificationKey, retireNotificationKey } from "./core/pushKeySync.js";

export const SERVICE_WORKER_URL = "/app/sw.js";
export const PUSH_OPEN_MESSAGE = "build.push.open";
const APP_PATH = "/app/";
const APP_LINK_PREFIX = "/app/#/";

// Decode an unpadded base64url string (the VAPID public key) to the raw bytes
// PushManager.subscribe expects as applicationServerKey. Pure.
export function urlBase64ToUint8Array(base64UrlString) {
  const padded =
    base64UrlString + "=".repeat((4 - (base64UrlString.length % 4)) % 4);
  const base64 = padded.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function pushSupported() {
  return (
    "serviceWorker" in navigator &&
    "PushManager" in window &&
    "Notification" in window
  );
}

// Register the worker (idempotent). Safe to call on every boot; the worker is
// cache-free so an update can never wedge the app.
export async function registerPushWorker() {
  if (!("serviceWorker" in navigator)) return null;
  return navigator.serviceWorker.register(SERVICE_WORKER_URL, { scope: "/app/" });
}

// The current state for the settings toggle: "unsupported" | "denied" |
// "enabled" | "disabled".
export async function pushState() {
  if (!pushSupported()) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const registration = await navigator.serviceWorker.getRegistration("/app/");
  const subscription = await registration?.pushManager.getSubscription();
  return subscription ? "enabled" : "disabled";
}

// Ask permission, subscribe this browser with the server's VAPID key, and store
// the subscription with the api.
export async function enablePush() {
  if (!pushSupported()) throw new Error("this browser does not support push");
  const permission = await Notification.requestPermission();
  if (permission !== "granted") throw new Error("notifications are blocked for this site");
  const registration = await registerPushWorker();
  await navigator.serviceWorker.ready;
  const vapidPublicKey = await fetchVapidPublicKey();
  const subscription = await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: urlBase64ToUint8Array(vapidPublicKey),
  });
  await subscribePush(subscription.toJSON());
  // In the background: a bridge that is slow or refuses never holds up (or
  // undoes) turning push on; the push just shows the generic copy.
  void armNotificationKey(subscription);
}

// Drop the browser subscription and tell the api to forget it.
export async function disablePush() {
  const registration = await navigator.serviceWorker.getRegistration("/app/");
  const subscription = await registration?.pushManager.getSubscription();
  if (!subscription) return;
  const endpoint = subscription.endpoint;
  await subscription.unsubscribe();
  void retireNotificationKey(endpoint);
  await unsubscribePush(endpoint);
}

/** Whether a message came from this origin's own active service worker: the
 *  worker at SERVICE_WORKER_URL that is the registration's active one or this
 *  page's controller. Nothing else may steer the app. */
export async function fromOwnWorker(source, container = navigator.serviceWorker) {
  if (typeof ServiceWorker === "undefined" || !(source instanceof ServiceWorker)) return false;
  if (source.scriptURL !== new URL(SERVICE_WORKER_URL, location.origin).href) return false;
  const registration = await container.getRegistration(APP_PATH);
  return source === registration?.active || source === container.controller;
}

/** The hash an in-app link routes to, or null for anything outside the app. */
export function appLinkHash(url) {
  return typeof url === "string" && url.startsWith(APP_LINK_PREFIX) ? url.slice(APP_PATH.length) : null;
}

async function followPushOpen(event, container, open) {
  if (event.data?.type !== PUSH_OPEN_MESSAGE) return;
  const hash = appLinkHash(event.data.url);
  if (!hash || !(await fromOwnWorker(event.source, container))) return;
  open(hash);
}

/**
 * Hear a notification click the service worker forwards to this open window
 * and route to its deep link without a reload. `open(hash)` does the routing;
 * the default sets `location.hash`, which the router re-renders from.
 */
export function installPushOpenListener({
  open = (hash) => {
    location.hash = hash;
  },
  container = globalThis.navigator?.serviceWorker,
} = {}) {
  if (!container) return () => {};
  const listener = (event) => void followPushOpen(event, container, open);
  container.addEventListener("message", listener);
  return () => container.removeEventListener("message", listener);
}
