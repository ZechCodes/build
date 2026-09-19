// Pure device-selection policy, shared by the gate (boot) and resume paths.

/**
 * Honor the user's sticky device choice only when that device is currently
 * online; otherwise return null ("any device"). Pinning a connect/resume to an
 * offline sticky device would leave every other machine unopened for ever.
 */
export function onlineStickyDeviceId(devices, selectedDeviceId) {
  const sticky = devices.find((d) => d.id === selectedDeviceId && d.status === "online");
  return sticky ? sticky.id : null;
}

/** What the account calls a device, or null when the list has never heard of
 *  it — the one place a device id is turned into words. */
export const deviceNameOf = (devices, deviceId) =>
  (devices || []).find((device) => device.id === deviceId)?.name || null;

/** The pick, or the first of them, or nobody: how home is chosen out of any
 *  list of candidates, so both tiers below choose it the same way. */
const pickOf = (candidates, selectedDeviceId) =>
  candidates.find((device) => device.id === selectedDeviceId)?.id || candidates[0]?.id || null;

/**
 * The home device: where creation goes, and the device a link that names none
 * is about.
 *
 * The sticky choice while that device can answer, else whichever device can —
 * first in the account's own order, so every surface that falls back falls back
 * to the same machine. Null when nothing is online: there is no home to name.
 *
 * `stillWorthAsking` narrows the candidates when a caller has it: a machine
 * this client has GIVEN UP on — one wearing a reason it cannot answer — loses
 * its claim to home, and the pick moves on down the account's order. A machine
 * that has simply not answered yet keeps its claim, so a boot race between two
 * bridges does not hand home to whichever one was quicker.
 *
 * Without that narrowing home is whatever the list says, and a machine listed
 * online that answers nothing takes every home-addressed call down with it:
 * creation and the composer refuse "Device not reachable" while the reader's
 * own workspace, routed to a different machine, works perfectly.
 *
 * When every candidate has been given up on the listing decides as before —
 * the refusal a surface shows then names a real machine rather than nobody.
 */
export function homeDeviceId(devices, selectedDeviceId, stillWorthAsking = null) {
  const online = (devices || []).filter((device) => device.status === "online");
  const asking = stillWorthAsking ? online.filter((device) => stillWorthAsking(device.id)) : online;
  return pickOf(asking, selectedDeviceId) || pickOf(online, selectedDeviceId);
}

/**
 * The machine creation goes to, as far as the account can say.
 *
 * Home while some device can be home, and otherwise the machine the user picked
 * — which cannot be home with nothing online, but is still where the work is
 * going the moment it is back. So a surface about creation can always name it.
 */
export const creationDeviceId = (devices, selectedDeviceId, stillWorthAsking = null) =>
  homeDeviceId(devices, selectedDeviceId, stillWorthAsking) || selectedDeviceId || null;

/**
 * How far past the api's own online window a machine's last beat may be and the
 * account's "offline" still be worth second-guessing.
 *
 * The api derives `online` from a 90 s heartbeat window and this client re-reads
 * the list every 15 s, so a bridge that has just come back is listed offline
 * for that long and no longer. Twice the window covers it with room to spare.
 */
const LISTING_MAY_LAG_MS = 180000;

/**
 * Whether the account calling this machine offline could be the listing being
 * behind rather than the machine being away.
 *
 * Read off `last_seen_at`, which `GET /api/devices` carries beside the status it
 * derived from it. A machine whose last beat was hours or days ago — or which
 * has never beaten at all — is not a listing that is lagging: it is a machine
 * that is off, and dialling it only makes the relay say so again. On
 * 2026-09-19 this client dialled a Mac mini last seen four days earlier, over
 * and over, and got `rejected session to device` every time.
 *
 * Against this browser's clock, so a badly skewed one declines the guess rather
 * than making a wrong one. That costs a dial the next presence read would make
 * a few seconds later anyway.
 */
export function listingCouldBeLagging(device, now = Date.now()) {
  const lastSeen = Date.parse(device?.last_seen_at ?? "");
  return Number.isFinite(lastSeen) && now - lastSeen <= LISTING_MAY_LAG_MS;
}
