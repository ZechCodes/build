// Scripted device approval for automated stacks (compose/CI).
//
// The bridge registers as *pending* with a pairing code; a human normally types
// that code into Settings → Devices. Here the bridge is started with a known
// code (BRIDGE_PAIRING_CODE) and this script plays the human: dummy-login,
// resolve the code to the pending device (lookup), then approve — binding the
// device to the QA user. Idempotent: exits 0 immediately if the user already
// owns an approved device.
//
// Usage: API_URL=http://127.0.0.1:8090 PAIRING_CODE=XXXX-YYYY node pair.mjs

import { loginWithDummy } from "./skrift-auth.mjs";

const apiUrl = process.env.API_URL || "http://127.0.0.1:8090";
const pairingCode = process.env.PAIRING_CODE;
const email = process.env.QA_EMAIL || "qa@localhost";
const deadlineMs = Number(process.env.PAIR_TIMEOUT_MS || 60000);

if (!pairingCode) {
  console.error("pair.mjs: PAIRING_CODE is required (must match the bridge's BRIDGE_PAIRING_CODE)");
  process.exit(2);
}

async function waitForApi(cookieless = `${apiUrl}/auth/dummy/login`, deadline = Date.now() + deadlineMs) {
  for (;;) {
    try {
      const response = await fetch(cookieless, { redirect: "manual" });
      if (response.status < 500) return;
    } catch {
      /* api not up yet */
    }
    if (Date.now() > deadline) throw new Error("api never became reachable");
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

async function main() {
  await waitForApi();
  const { cookie } = await loginWithDummy(apiUrl, { email, name: "QA" });
  const authed = (path, init = {}) =>
    fetch(`${apiUrl}${path}`, {
      ...init,
      headers: { "Content-Type": "application/json", Cookie: cookie, ...(init.headers || {}) },
    });

  const owned = await authed("/api/devices");
  if (owned.ok) {
    const { devices } = await owned.json();
    if (devices.length > 0) {
      console.log(`pair.mjs: ${email} already owns ${devices.length} approved device(s) — nothing to do`);
      return;
    }
  }

  // The bridge may still be registering: poll lookup until the code resolves.
  const deadline = Date.now() + deadlineMs;
  let device = null;
  for (;;) {
    const lookup = await authed("/api/devices/lookup", {
      method: "POST",
      body: JSON.stringify({ code: pairingCode }),
    });
    if (lookup.ok) {
      device = await lookup.json();
      break;
    }
    if (lookup.status !== 404) throw new Error(`lookup failed: HTTP ${lookup.status}`);
    if (Date.now() > deadline) throw new Error("no pending device for the pairing code (is the bridge up with BRIDGE_PAIRING_CODE set?)");
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  console.log(`pair.mjs: pending device ${device.device_id} (${device.name}) fingerprint ${device.fingerprint}`);

  const approve = await authed("/api/devices/approve", {
    method: "POST",
    body: JSON.stringify({ code: pairingCode }),
  });
  if (!approve.ok) throw new Error(`approve failed: HTTP ${approve.status}`);
  console.log(`pair.mjs: approved ${device.device_id} for ${email}`);
}

main().catch((error) => {
  console.error(`pair.mjs: ${error.message}`);
  process.exit(1);
});
