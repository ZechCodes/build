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
  if (e.target === $("#scrim")) $("#scrim").classList.remove("show");
};

boot();
