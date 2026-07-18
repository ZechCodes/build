// Entry point: fonts + styles, shell wiring, then the connection gate.

import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/inter/800.css";
import "./styles.css";

import { $ } from "./dom.js";
import { App, go, initRouter } from "./app.js";
import { initDevicePicker } from "./devices.js";
import { registerPushWorker } from "./push.js";
import { requestSheetDismiss } from "./core/sheetDismiss.js";
import { boot } from "./views/gate.js";

// Keep the (cache-free) push worker current on every boot so notification
// clicks keep working after deploys. /app/sw.js only exists on the real app
// origin — skip under the vite dev server.
if (location.pathname.startsWith("/app")) {
  registerPushWorker().catch(() => {});
}

initRouter();
initDevicePicker();

$(".logo").onclick = () => go({ name: "notifications" });
$("#nav-notif").onclick = () => go({ name: "notifications" });
$("#nav-account").onclick = () => go({ name: "settings" });
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
