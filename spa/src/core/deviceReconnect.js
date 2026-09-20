// When one machine can answer again — the subscription a surface waits on
// instead of polling after a read was lost with the wire.
//
// Two things have to move before a machine can answer: the registry adopts a
// session for it (core/deviceContexts.js) and the supervisor stands down
// (core/deviceRecovery.js). Neither alone is the answer, so both are listened
// to and both are asked. That pairing is already the house pattern — every
// surface that greys itself over a lost machine subscribes to exactly these
// two (core/deviceNotice.js) — and this is it, named, so the surfaces that
// hold a cached copy quietly all wait the same way.
//
// The policy that uses this lives in core/transientRead.js, which is pure and
// takes a `watch`; `deviceWatch` below is that watch over the real registry.

import { canAnswer, contextFor, onDeviceStateChanged } from "./deviceContexts.js";
import { deviceRecoverySnapshot, onDeviceRecoveryChanged } from "../connection.js";
import { isRecovering } from "./connectionStatusModel.js";

/** Hear whenever anything about any machine's reachability moves. Returns the
 *  unsubscribe, which takes both subscriptions with it. */
export function onDeviceMoved(fn) {
  const stops = [onDeviceStateChanged(fn), onDeviceRecoveryChanged(fn)];
  return () => stops.splice(0).forEach((stop) => stop());
}

/** Whether something is being done about this machine right now, in the
 *  connection model's own reading of its recovery record — the same reading
 *  the ring in the header is drawn from. */
export const deviceIsReconnecting = (deviceId) => isRecovering(deviceRecoverySnapshot(deviceId));

/**
 * Whether this machine cannot answer right now.
 *
 * A machine mid-reconnect counts as away even while a context for it still
 * stands: the session under it is the one that just died, and a read sent into
 * it would fail the same way the last one did.
 */
export const deviceIsAway = (deviceId) => !canAnswer(contextFor(deviceId)) || deviceIsReconnecting(deviceId);

/**
 * Do this once, when this machine can answer again.
 *
 * One shot: it unsubscribes itself before calling, so a surface cannot be run
 * twice by the two subscriptions announcing the same reconnect. Returns the
 * unsubscribe, for a surface that goes away first.
 */
export function onDeviceReachable(deviceId, fn) {
  let done = false;
  let off = null;
  const stop = () => {
    done = true;
    const taking = off;
    off = null;
    taking?.();
  };
  // `done` and not merely the unsubscribe: the registry and the supervisor
  // both announce the same reconnect, and a broadcast already in flight when
  // this unsubscribes still reaches what it had in hand.
  off = onDeviceMoved(() => {
    if (done || deviceIsAway(deviceId)) return;
    stop();
    fn();
  });
  return stop;
}

/** One machine as core/transientRead.js asks about it. */
export const deviceWatch = (deviceId) => ({
  away: () => deviceIsAway(deviceId),
  reconnecting: () => deviceIsReconnecting(deviceId),
  moved: onDeviceMoved,
});
