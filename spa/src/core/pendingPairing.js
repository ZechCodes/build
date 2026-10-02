// Devices the reader has just approved, until each answers (#321).
//
// Approving a pairing used to land the reader on "watching for a device to
// come online…" for half a minute or more: the bridge beat late, and the page
// read the account every three seconds (gated) or fifteen (in the app). So an
// approve is remembered here, and for PAIRING_WINDOW_MS the account is read
// every second, wherever the reader is, and the device is named while it comes
// up. It is what this page did, not a read of any connection: a wait ends when
// the cached account list calls the device online, or when its window runs
// out, and either way the surfaces that named it hear so and repaint.
//
// One entry per device, so approving a second machine while the first is still
// coming up waits for both. Nothing here survives a reload: a page loaded
// afterwards reads the account at its own cadence, which is what it did before.

/** How long after an approve the device is expected. A bridge beats within a
 *  second of its relay socket coming up and every 30 s after, so one that has
 *  not been seen in 90 s is not coming up by itself. */
export const PAIRING_WINDOW_MS = 90_000;

/** How often the account is read while a just-approved device is expected. */
export const PAIRING_CADENCE_MS = 1000;

/** deviceId → { deviceId, name, approvedAt, windowEnds } */
const pending = new Map();
const listeners = new Set();

/** Tell every listener; one that throws (a surface torn down mid-repaint) is
 *  reported and does not keep the rest from hearing. */
const announce = () => {
  for (const listener of [...listeners]) {
    try {
      listener();
    } catch (error) {
      console.warn("a pending-pairing listener threw:", error);
    }
  }
};

function endPairing(deviceId) {
  const pairing = pending.get(deviceId);
  if (!pairing) return false;
  clearTimeout(pairing.windowEnds);
  pending.delete(deviceId);
  return true;
}

/** The reader approved `device` (the lookup's answer: `device_id`, `name`). */
export function notePairingApproved(device, now = Date.now()) {
  const deviceId = device?.device_id;
  if (!deviceId) return;
  endPairing(deviceId);
  const windowEnds = setTimeout(() => {
    if (endPairing(deviceId)) announce();
  }, PAIRING_WINDOW_MS);
  pending.set(deviceId, { deviceId, name: device.name || "", approvedAt: now, windowEnds });
  announce();
}

/** The devices still being waited for, oldest approve first. */
export const pairingsConnecting = () =>
  [...pending.values()].map(({ deviceId, name, approvedAt }) => ({ deviceId, name, approvedAt }));

/** Whether a device as the account lists it is one being waited for: approved
 *  here, inside its window, and not yet called online. */
export const isPairingConnecting = (device) => pending.has(device?.id) && device.status !== "online";

/** The account list was taken up: every device it calls online has landed. */
export function pairingsLandedIn(devices) {
  let landed = false;
  for (const device of devices) {
    if (device.status === "online" && endPairing(device.id)) landed = true;
  }
  if (landed) announce();
}

/** Hear the devices being waited for change: approved, landed, or run out. */
export function onPairingChanged(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** How often to read the account: every second while a device is awaited,
 *  `slowMs` otherwise. */
export const accountReadCadence = (slowMs) => (pending.size ? Math.min(slowMs, PAIRING_CADENCE_MS) : slowMs);

/** Forget everything (tests, account reset). */
export function resetPendingPairing() {
  for (const deviceId of [...pending.keys()]) endPairing(deviceId);
}
