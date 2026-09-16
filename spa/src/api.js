// Same-origin api calls (Skrift-session authed). These are the only plain-HTTP
// surfaces — everything task-shaped rides the E2EE session instead.

export async function fetchGatewayToken() {
  const response = await fetch("/api/gateway-token", { method: "POST" });
  if (!response.ok) throw new Error("could not mint a gateway token");
  return (await response.json()).token;
}

export async function fetchDevices() {
  try {
    return (await fetch("/api/devices").then((r) => r.json())).devices || [];
  } catch {
    return [];
  }
}

export async function lookupDevice(code) {
  const response = await fetch("/api/devices/lookup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!response.ok) throw new Error("no pending device for that code");
  return response.json();
}

export async function approveDevice(code) {
  const response = await fetch("/api/devices/approve", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!response.ok) throw new Error(await refusalDetail(response, "approve failed"));
}

/** Where to get the bridge: the install one-liner and one link per platform.
 *  Every URL in it is the api's, so a dev stack answers with its own origin and
 *  the client never carries a hardcoded release address. */
export async function fetchDownloads() {
  const response = await fetch("/app/downloads");
  if (!response.ok) throw new Error(await refusalDetail(response, "downloads are not available"));
  return response.json();
}

/** A fresh install one-liner. The token inside the line lives ten minutes and
 *  is spent by the download it authorizes, so the page that shows the line asks
 *  for a new one rather than handing over a stale one. */
export async function mintInstallCommand() {
  const response = await fetch("/app/downloads/token", { method: "POST" });
  if (!response.ok) throw new Error(await refusalDetail(response, "could not refresh the install line"));
  return response.json();
}

/** The sentence the api refused with, when it gave one — the device cap says
 *  what to do about itself — else `fallback`. */
async function refusalDetail(response, fallback) {
  try {
    const detail = (await response.json())?.detail;
    return typeof detail === "string" && detail ? detail : fallback;
  } catch {
    return fallback;
  }
}

export async function revokeDevice(deviceId) {
  const response = await fetch(`/api/devices/${deviceId}/revoke`, { method: "POST" });
  if (!response.ok) throw new Error("revoke failed");
}

export async function renameDevice(deviceId, name) {
  const response = await fetch(`/api/devices/${deviceId}/rename`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  });
  if (!response.ok) throw new Error(await refusalDetail(response, "name could not be saved"));
  return response.json();
}

export async function fetchIceServers() {
  const response = await fetch("/api/rtc/ice-servers", { method: "POST" });
  if (!response.ok) throw new Error("could not mint ICE servers");
  return (await response.json()).iceServers;
}

export async function fetchVapidPublicKey() {
  const response = await fetch("/api/push/vapid-public-key");
  if (!response.ok) throw new Error("push notifications are not configured on the server");
  return (await response.json()).public_key;
}

export async function subscribePush(subscription) {
  const response = await fetch("/api/push/subscribe", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(subscription),
  });
  if (!response.ok) throw new Error("could not store the push subscription");
}

export async function unsubscribePush(endpoint) {
  const response = await fetch("/api/push/unsubscribe", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ endpoint }),
  });
  if (!response.ok) throw new Error("could not remove the push subscription");
}
