// Web-push subscription management. The pushes themselves are content-free by
// design (E2EE — the server never sees task content), so all this does is hand
// the browser's push subscription to the api and take it back.
//
// The service worker (public/sw.js, served same-origin at /app/sw.js) renders
// every push generically and never caches anything.

import {
  fetchVapidPublicKey,
  subscribePush,
  unsubscribePush,
} from "./api.js";

export const SERVICE_WORKER_URL = "/app/sw.js";

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
}

// Drop the browser subscription and tell the api to forget it.
export async function disablePush() {
  const registration = await navigator.serviceWorker.getRegistration("/app/");
  const subscription = await registration?.pushManager.getSubscription();
  if (!subscription) return;
  const endpoint = subscription.endpoint;
  await subscription.unsubscribe();
  await unsubscribePush(endpoint);
}
