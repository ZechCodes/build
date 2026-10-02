// A device the reader has just approved, until it answers (#321).
//
// Approving a pairing used to land the reader on "watching for a device to
// come online…" for half a minute or more: the bridge beat late, and the page
// read the account every three seconds (gated) or fifteen (in the app). So the
// approve is remembered here, and for PAIRING_WINDOW_MS the account is read
// every second, wherever the reader is, and the device is named while it comes
// up. It is what this page did, not a read of any connection: the surfaces
// that paint it paint the account list beside it, from the cache.
//
// Past the window the device is still remembered, as late, so the waiting
// screen can say what to check rather than only that it is waiting. The phase
// is read off the clock, not announced: the polls that pace themselves by it
// tick every second while it lasts and see it end.

/** How long after an approve the device is expected. A bridge beats within a
 *  second of its relay socket coming up and every 30 s after, so one that has
 *  not been seen in 90 s is not coming up by itself. */
export const PAIRING_WINDOW_MS = 90_000;

/** How often the account is read while a just-approved device is expected. */
export const PAIRING_CADENCE_MS = 1000;

let held = null;
const listeners = new Set();

const announce = () => {
  for (const listener of [...listeners]) listener(pairingState());
};

/** The reader approved `device` (the lookup's answer: `device_id`, `name`). */
export function notePairingApproved(device, now = Date.now()) {
  if (!device?.device_id) return;
  held = { deviceId: device.device_id, name: device.name || "", approvedAt: now };
  announce();
}

/** The device being waited for, or null: `phase` is "connecting" inside the
 *  window and "late" after it. */
export function pairingState(now = Date.now()) {
  if (!held) return null;
  return { ...held, phase: now - held.approvedAt < PAIRING_WINDOW_MS ? "connecting" : "late" };
}

/** Whether `deviceId` is the device a pairing is still connecting. */
export const isPairingConnecting = (deviceId, now = Date.now()) => {
  const state = pairingState(now);
  return Boolean(state && state.phase === "connecting" && state.deviceId === deviceId);
};

/** The device answered: nothing is pending any more. */
export function pairingLanded(deviceId) {
  if (!held || held.deviceId !== deviceId) return;
  held = null;
  announce();
}

/** End the wait if one of `deviceIds` — the machines answering now — is the
 *  device it is for. */
export function pairingLandedAmong(deviceIds) {
  if (held && deviceIds.includes(held.deviceId)) pairingLanded(held.deviceId);
}

/** Hear the pending device change: approved, or landed. */
export function onPairingChanged(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** How often to read the account: every second while a device is connecting,
 *  `slowMs` otherwise. */
export const accountReadCadence = (slowMs, now = Date.now()) =>
  pairingState(now)?.phase === "connecting" ? Math.min(slowMs, PAIRING_CADENCE_MS) : slowMs;

/** Forget everything (tests, account reset). */
export function resetPendingPairing() {
  held = null;
}
