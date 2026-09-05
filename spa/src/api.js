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
  if (!response.ok) throw new Error("approve failed");
}

export async function revokeDevice(deviceId) {
  const response = await fetch(`/api/devices/${deviceId}/revoke`, { method: "POST" });
  if (!response.ok) throw new Error("revoke failed");
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
