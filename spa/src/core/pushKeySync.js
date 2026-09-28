// The notification key, told to the bridges that seal pushes to it (#200).
//
// A bridge seals a push's content to this browser's notification public key
// (pushKeys.js), and learns that key only here, over the E2EE session:
// `push.registerKey` on every bridge that announces it when push is turned on,
// again after every greeting (an idempotent upsert, so a bridge that restarted
// or pruned the key has it back), and `push.revokeKey` when push is turned off.
//
// All of it is a background side effect: it paints nothing, and a refusal only
// warns — it can never stand in the way of turning push on.

import { bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";
import { canAnswer, contextFor, liveContexts } from "./deviceContexts.js";
import { deletePushKey, ensurePushKey, subscriptionIdOf } from "../pushKeys.js";

const carries = (deviceId, verb) => bridgeCapabilities(deviceId)?.push?.[verb] === true;

const warn = (what) => (error) => {
  console.warn("push:", what, error?.message || String(error));
};

/** This browser's push subscription, or null. */
export async function browserSubscription() {
  const registration = await globalThis.navigator?.serviceWorker?.getRegistration("/app/");
  return (await registration?.pushManager?.getSubscription()) || null;
}

/** The key for a subscription, made (and the stale ones deleted) when its sid
 *  has none. Null with no subscription. */
export async function keyForSubscription(subscription) {
  if (!subscription?.endpoint) return null;
  return ensurePushKey(await subscriptionIdOf(subscription.endpoint));
}

async function registerOn(context, key) {
  if (!canAnswer(context) || !carries(context.deviceId, "registerKey")) return;
  await context.rpc("push.registerKey", { subscription_id: key.sid, public_key: key.publicKey });
}

/** Register the key with every connected bridge that announces the verb. */
export async function registerKeyEverywhere(key) {
  await Promise.all(liveContexts().map((context) =>
    registerOn(context, key).catch(warn("a bridge refused the notification key"))));
}

/** After `enablePush` subscribed: make the key and hand it to every bridge. */
export async function armNotificationKey(subscription) {
  try {
    const key = await keyForSubscription(subscription);
    if (key) await registerKeyEverywhere(key);
  } catch (error) {
    warn("could not set up the notification key")(error);
  }
}

/** After `disablePush`: every bridge forgets the key, then this browser does. */
export async function retireNotificationKey(endpoint) {
  try {
    const sid = await subscriptionIdOf(endpoint);
    await Promise.all(liveContexts()
      .filter((context) => canAnswer(context) && carries(context.deviceId, "revokeKey"))
      .map((context) => context.rpc("push.revokeKey", { subscription_id: sid })
        .catch(warn("a bridge refused to revoke the notification key"))));
    await deletePushKey(sid);
  } catch (error) {
    warn("could not retire the notification key")(error);
  }
}

/** A bridge greeted: register this browser's key with it again. A key made
 *  just now (the subscription rotated) goes to every bridge. */
export async function registerAfterGreeting(deviceId, subscription = browserSubscription) {
  if (!carries(deviceId, "registerKey")) return;
  const key = await keyForSubscription(await subscription());
  if (!key) return;
  if (key.created) await registerKeyEverywhere(key);
  else await registerOn(contextFor(deviceId), key);
}

let stopSync = null;

/** Register after every greeting from now on. Returns the stop; a second
 *  start replaces the first. */
export function startPushKeySync({ subscription = browserSubscription } = {}) {
  stopSync?.();
  const stopListening = onBridgeGreeted((deviceId) => {
    void registerAfterGreeting(deviceId, subscription).catch(warn("could not register the notification key"));
  });
  stopSync = () => {
    stopListening();
    stopSync = null;
  };
  return stopSync;
}
