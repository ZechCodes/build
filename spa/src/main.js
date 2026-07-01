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
import { initTerminalDrawer } from "./terminal/drawer.js";
import { boot } from "./views/gate.js";

initRouter();
initDevicePicker();
initTerminalDrawer();

$("#nav-board").onclick = () => go({ name: "board" });
$("#nav-notif").onclick = () => go({ name: "notifications" });
$("#nav-settings").onclick = () => go({ name: "settings" });
$("#scrim").onclick = (e) => {
  if (e.target === $("#scrim")) $("#scrim").classList.remove("show");
};

boot();
