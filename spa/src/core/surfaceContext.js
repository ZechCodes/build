// The machine a route surface paints under, asked the one way every surface
// asks it.
//
// Rendering never asks the connection (ARCHITECTURE.md, Render from cache): a
// surface paints what the records hold, and the pulls and pushes that follow
// write over it. So whether the machine can answer right now is no part of
// whether its surface stands up. The only question is whether this client
// knows the machine at all.

import { contextFor, knownDeviceContext } from "./deviceContexts.js";
import { deviceFeedNow } from "./feedRows.js";

/**
 * The context a route's surface stands on, or null when nothing here has ever
 * held the machine its link names.
 *
 * A machine has a context once it has answered in this tab, and keeps it
 * through an outage. On a cold reload nothing has answered yet, but the
 * records the last session left still hold the machine: a session-less context
 * stands in for it (it answers nothing, and the session retargets it when it
 * lands), so the surface paints from disk rather than waiting on the wire.
 */
export function surfaceContext(route) {
  const deviceId = route?.deviceId;
  if (!deviceId) return null;
  return contextFor(deviceId) || (deviceFeedNow(deviceId) ? knownDeviceContext(deviceId) : null);
}
