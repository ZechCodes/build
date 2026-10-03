// Shared account and route state, independent of the app shell and transport.
// Core device/cache modules read this without importing views or connection.js.

import { createViewingContext } from "./core/viewingContext.js";

export const SELECTED_DEVICE_KEY = "build.selectedDeviceId";
// What the app holds that is nobody's machine in particular: where the reader
// is standing, what the mounted view owes a teardown, and the account's device
// list. Everything that belongs to a machine — its transport, its cache scope,
// its drafts, whether it can answer — is held on that machine's context
// (core/deviceContexts.js), because the app is on all of them at once.
export const App = {
  accountEpoch: 0,
  viewingContext: createViewingContext({ enabled: false }),
  route: { name: "inbox" },
  poll: null, // the current view's change watcher (core/changeEvents.js)
  viewDispose: null, // the current view's teardown (terminal panes, observers)
  routeLeaveGuard: null, // async veto owned by the mounted view (for unsaved work)

  gated: true, // gate screens own #root until a session is live
  updateAvailable: false, // the served-version watcher found a newer bundle than this one
  devices: [], // last GET /api/devices, re-read by the presence poll (devices.js)
  selectedDeviceId: localStorage.getItem(SELECTED_DEVICE_KEY) || null,
  // Which machines the inbox, the projects face and the project menu list —
  // null for all of them. It narrows lists and nothing else: no route, no
  // session and no creation reads it.
  deviceFilter: null,

  // One-shot: set right before navigating to a branch just cut from the
  // toolbar's create form, so the branch view knows to focus the rail's
  // composer the moment it exists — read and cleared by the very next
  // renderBranch, which is what that navigation lands on. The route/hash
  // can't carry this itself (routeFromHash/hashFromRoute only round-trip
  // name/projectId/branch/tab).
  focusComposerOnMount: false,

};
