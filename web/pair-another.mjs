// Approve a SECOND (third, fourth…) device for the QA user by pairing code.
//
// web/pair.mjs is the scripted stand-in for a human typing a code into
// Settings → Devices, and it is idempotent the way an automated stack wants: it
// exits as soon as the account owns an approved device, so running it again for
// another bridge does nothing. The multi-device browser pass needs the opposite
// — one account, two machines — so this one skips that guard and approves the
// code it is given however many devices the account already has. Everything
// else is the same flow: dummy-login, redeem the invite, lookup, approve.
//
// Usage (from web/, with deploy/compose.two-bridges.yml up):
//   API_URL=http://localhost:8090 PAIRING_CODE=COMPOSE-PAIR-2 node pair-another.mjs
//
// | Env            | Default                 | What it is                      |
// |----------------|-------------------------|---------------------------------|
// | API_URL        | http://localhost:8090   | the app, as the host reaches it |
// | PAIRING_CODE   | COMPOSE-PAIR-2          | the bridge's BRIDGE_PAIRING_CODE|
// | INVITE_TOKEN   | COMPOSE-INVITE          | BUILD_DEV_INVITE_TOKEN          |
// | QA_EMAIL       | qa@localhost            | who the device is bound to      |
// | PAIR_TIMEOUT_MS| 90000                   | how long to wait for the code   |

import { loginWithDummy } from "./skrift-auth.mjs";

const apiUrl = process.env.API_URL || "http://localhost:8090";
const pairingCode = process.env.PAIRING_CODE || "COMPOSE-PAIR-2";
const email = process.env.QA_EMAIL || "qa@localhost";
const inviteToken = process.env.INVITE_TOKEN || "COMPOSE-INVITE";
const deadlineMs = Number(process.env.PAIR_TIMEOUT_MS || 90000);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function main() {
  const { cookie } = await loginWithDummy(apiUrl, { email, name: "QA" });
  const authed = (path, init = {}) =>
    fetch(`${apiUrl}${path}`, {
      ...init,
      headers: { "Content-Type": "application/json", Cookie: cookie, ...(init.headers || {}) },
    });

  // Build is invite-only and every device route sits behind alpha membership.
  // Redeeming twice is not an error — the second visit answers the "already
  // used" page — so this runs whether or not pair.mjs got there first.
  await fetch(`${apiUrl}/invite/${inviteToken}`, { redirect: "manual", headers: { Cookie: cookie } });

  // The bridge may still be registering: poll lookup until the code resolves.
  const deadline = Date.now() + deadlineMs;
  let device = null;
  for (;;) {
    const lookup = await authed("/api/devices/lookup", { method: "POST", body: JSON.stringify({ code: pairingCode }) });
    if (lookup.ok) {
      device = await lookup.json();
      break;
    }
    if (lookup.status !== 404) throw new Error(`lookup failed: HTTP ${lookup.status}`);
    if (Date.now() > deadline) throw new Error(`no pending device for ${pairingCode} (is the second bridge up?)`);
    await sleep(1000);
  }
  console.log(`pair-another.mjs: pending device ${device.device_id} (${device.name}) fingerprint ${device.fingerprint}`);

  const approve = await authed("/api/devices/approve", { method: "POST", body: JSON.stringify({ code: pairingCode }) });
  if (!approve.ok) throw new Error(`approve failed: HTTP ${approve.status}`);

  const { devices } = await (await authed("/api/devices")).json();
  console.log(`pair-another.mjs: approved ${device.device_id} for ${email} — ${devices.length} device(s) now:`);
  for (const owned of devices) console.log(`  ${owned.name} (${owned.id}) ${owned.status}`);
}

main().catch((error) => {
  console.error(`pair-another.mjs: ${error.message}`);
  process.exit(1);
});
