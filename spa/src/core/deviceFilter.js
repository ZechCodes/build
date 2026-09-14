// Which machines the inbox shows.
//
// The rail, the projects face and the toolbar's project menu are lists of the
// whole account's work; the picker narrows them to one machine when the account
// has grown too wide to read. It narrows nothing else: a surface open on a
// machine the filter hides stays open, and no route is touched — the filter is
// about what is listed, not about where you are standing.
//
// So it is one function from a snapshot to a snapshot whose "all devices" case
// is the identity, applied where each list painter stores what it was handed.
// No painter asks whether a filter is set.

import { App, DEVICE_FILTER_KEY } from "../app.js";
import { deliverFeed } from "./taskFeed.js";

/**
 * One machine's rows out of the merge, or the merge itself when the filter
 * names no machine.
 *
 * Every collection the snapshot carries is filtered the same way — each row was
 * stamped with the device that answered for it (core/feedMerge.js) — so a
 * collection added to the feed later is narrowed without this being touched.
 */
export function filterByDevice(snapshot, deviceFilter) {
  if (!deviceFilter || !snapshot) return snapshot;
  const shown = { ...snapshot };
  for (const [field, value] of Object.entries(snapshot)) {
    if (Array.isArray(value)) shown[field] = value.filter((row) => row.deviceId === deviceFilter);
  }
  if (snapshot.devices) shown.devices = onlyDevice(snapshot.devices, deviceFilter);
  return shown;
}

/** The `devices` map narrowed the same way: a surface that reads one machine's
 *  slice out of a filtered snapshot must not find another machine's. */
function onlyDevice(devices, deviceId) {
  const view = devices[deviceId];
  return view ? { [deviceId]: view } : {};
}

/**
 * Remember which machines the inbox shows, and repaint the lists.
 *
 * The pick is this browser's, not the account's, and nothing about any bridge
 * has changed — so the three lists are handed the merge they already have,
 * through the subscription each of them already paints from.
 */
export function rememberDeviceFilter(deviceId) {
  App.deviceFilter = deviceId || null;
  if (App.deviceFilter) localStorage.setItem(DEVICE_FILTER_KEY, App.deviceFilter);
  else localStorage.removeItem(DEVICE_FILTER_KEY);
  deliverFeed();
}
