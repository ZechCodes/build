// Entry point: fonts + styles, shell wiring, then the connection gate.

import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/inter/800.css";
import "./styles.css";

import { $ } from "./dom.js";
import { App, followNotificationLink, go, initRouter } from "./app.js";
import { initDevicePicker } from "./devices.js";
import { installPushOpenListener, registerPushWorker } from "./push.js";
import { startPushKeySync } from "./core/pushKeySync.js";
import { createVersionWatcher, fetchServedVersion } from "./core/version.js";
import { requestSheetDismiss } from "./core/sheetDismiss.js";
import { installTheme } from "./core/theme.js";
import { initCompose } from "./core/composeView.js";
import { boot, openPairingLink } from "./views/gate.js";
import { watchPairLinks } from "./core/pairLink.js";
import { mountConnectionStatus } from "./connectionStatus.js";
import { startUserPresence } from "./core/userPresence.js";

// Before anything renders: index.html's inline stamp beat the first paint, this
// takes ownership of the same attribute and keeps following the OS while the
// preference says "system".
installTheme();

// Keep the (cache-free) push worker current on every boot so notification
// clicks keep working after deploys. /app/sw.js only exists on the real app
// origin — skip under the vite dev server.
if (location.pathname.startsWith("/app")) {
  registerPushWorker().catch(() => {});
  // A notification clicked while this window is open routes here, without a
  // reload (public/sw.js posts `build.push.open`).
  installPushOpenListener({ open: followNotificationLink });
  // Watch the served frontend version and offer one reload when this bundle
  // falls behind a deploy — the resume check is what reaches a PWA that slept
  // through it. (start() is a no-op for dev builds.)
  const versionWatcher = createVersionWatcher({
    currentVersion: import.meta.env.VITE_BUILD_VERSION || "dev",
    fetchVersion: fetchServedVersion,
    onStale: () => {
      App.updateAvailable = true; // what lets the app-behind version gate offer its reload
      $("#verbar").hidden = false;
    },
  });
  versionWatcher.start();
  // The push accelerant: a deploy announcement from the worker re-checks now
  // instead of on the next interval, so open clients see the banner in seconds.
  $("#verbar-reload").onclick = () => location.reload();
}

// Before the router: an approve link's code leaves the address before any
// route reads it, and its approve screen opens over whatever the page shows.
watchPairLinks(window, openPairingLink);
initRouter();
initDevicePicker();
// Load captures saved by earlier builds so they can flush after connection.
initCompose();
mountConnectionStatus();

$("#nav-account").onclick = () => go({ name: "account", page: "settings" });
$("#scrim").onclick = (e) => {
  if (e.target === $("#scrim")) requestSheetDismiss();
};
// Escape dismisses the open sheet (with a discard-confirm when there's a draft).
// confirm.js installs a capture-phase Escape handler that stops propagation
// while a modal is open, and requestSheetDismiss double-guards with
// isConfirmOpen(), so the modal always wins when both are on screen.
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") requestSheetDismiss();
});

boot();
// The user arriving, told to each bridge: where "Done since you left" starts.
startUserPresence();
// Every bridge that greets is handed this browser's notification key again, so
// it can seal what a push says (#200).
startPushKeySync();
