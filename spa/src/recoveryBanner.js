import { App } from "./app.js";
import { deviceRecoverySnapshot, onDeviceRecoveryChanged } from "./connection.js";
import { esc } from "./core/text.js";

let stopListening = null;
let countdownTimer = null;
const announcedEpisodes = new Set();

const visibleRecoveries = () =>
  deviceRecoverySnapshot().filter((state) => state.status !== "idle" && state.failedAttempts > 0);

const deviceName = (deviceId) => App.devices.find((device) => device.id === deviceId)?.name || "device";

function bannerText(state, now = Date.now()) {
  const seconds = Math.max(0, Math.ceil((state.nextAttemptAt - now) / 1000));
  const suffix = state.status === "waiting" && seconds > 0 ? ` in ${seconds}s` : "";
  return `Reconnecting to ${deviceName(state.deviceId)}${suffix}`;
}

function paint() {
  const host = document.getElementById("recovery-banners");
  const live = document.getElementById("recovery-announcement");
  if (!host || !live) return;
  const states = visibleRecoveries();
  host.innerHTML = states.map((state) => `<div class="recovery-banner">${esc(bannerText(state))}</div>`).join("");

  const activeIds = new Set(states.map((state) => state.deviceId));
  for (const id of announcedEpisodes) if (!activeIds.has(id)) announcedEpisodes.delete(id);
  const fresh = states.filter((state) => !announcedEpisodes.has(state.deviceId));
  if (fresh.length) {
    fresh.forEach((state) => announcedEpisodes.add(state.deviceId));
    live.textContent = `Reconnecting to ${fresh.map((state) => deviceName(state.deviceId)).join(" and ")}`;
  } else if (!states.length) {
    live.textContent = "";
  }
  clearInterval(countdownTimer);
  countdownTimer = states.some((state) => state.status === "waiting") ? setInterval(paint, 1000) : null;
}

export function mountRecoveryBanners() {
  stopListening?.();
  stopListening = onDeviceRecoveryChanged(paint);
  paint();
  return unmountRecoveryBanners;
}

export function unmountRecoveryBanners() {
  stopListening?.();
  stopListening = null;
  clearInterval(countdownTimer);
  countdownTimer = null;
  announcedEpisodes.clear();
  const host = document.getElementById("recovery-banners");
  if (host) host.innerHTML = "";
  const live = document.getElementById("recovery-announcement");
  if (live) live.textContent = "";
}

export { bannerText };
