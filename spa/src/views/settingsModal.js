import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { onDeviceStateChanged } from "../core/deviceContexts.js";
import { renderSettings } from "./settings.js";
import { renderDeviceSettings } from "./deviceSettings.js";
import { renderArchive } from "./archive.js";
import { paintDevicePicker } from "../devices.js";

export function isSettingsRoute(route) {
  return route.name === "device" || route.name === "account";
}

/** Settings owns an overlay; the previous work surface remains behind it. */
export function renderSettingsModal(returnRoute = { name: "inbox" }) {
  const previousFocus = document.activeElement;
  const background = document.querySelector("#shell") || document.querySelector("#root");
  const wasInert = background.inert;
  const scrim = document.createElement("div");
  scrim.className = "settings-scrim";
  scrim.innerHTML = `<section class="settings-modal" role="dialog" aria-modal="true" aria-label="Settings">
    <header><h2>Settings</h2><button class="btn" data-settings-close aria-label="Close settings">✕</button></header>
    <nav class="settings-sidebar" aria-label="Settings sections"></nav>
    <div class="settings-content"></div>
  </section>`;
  document.body.appendChild(scrim);
  background.inert = true;
  document.body.classList.add("settings-open");
  let disposePanel = null;
  let generation = 0;
  let selected = App.route.name === "device" ? App.route.id : App.route.page === "archive" ? "archive" : null;
  const content = scrim.querySelector(".settings-content");
  const sidebar = scrim.querySelector("nav");
  const close = () => go(returnRoute);
  let sidebarHtml = null;
  const focusedSidebarEntry = () => {
    if (!sidebar.contains(document.activeElement)) return null;
    if (document.activeElement.matches("[data-local]")) return { kind: "local" };
    if (document.activeElement.matches(".settings-archive")) return { kind: "archive" };
    return document.activeElement.dataset.settingsDevice
      ? { kind: "device", id: document.activeElement.dataset.settingsDevice }
      : null;
  };
  const restoreSidebarFocus = (entry) => {
    if (entry?.kind === "local") sidebar.querySelector("[data-local]")?.focus();
    if (entry?.kind === "archive") sidebar.querySelector(".settings-archive")?.focus();
    if (entry?.kind === "device") {
      [...sidebar.querySelectorAll("[data-settings-device]")]
        .find((button) => button.dataset.settingsDevice === entry.id)
        ?.focus();
    }
  };
  const paintSidebar = () => {
    const html = `<button class="btn" data-local ${selected === null ? 'aria-current="page"' : ""}>Local settings</button>
      <h3>Devices</h3>${App.devices.map((device) => `<button class="btn" data-settings-device="${esc(device.id)}" ${selected === device.id ? 'aria-current="page"' : ""}>${esc(device.name)}<small>${esc(device.status)}</small></button>`).join("")}
      ${App.devices.length ? "" : '<p class="dim">No paired devices</p>'}
      <a class="btn settings-archive" href="#/account/archive" ${selected === "archive" ? 'aria-current="page"' : ""}>Archived work</a>`;
    if (sidebarHtml === html) return;
    const focusedEntry = focusedSidebarEntry();
    sidebarHtml = html;
    sidebar.innerHTML = html;
    sidebar.querySelector("[data-local]").onclick = () => void select(null);
    sidebar.querySelectorAll("[data-settings-device]").forEach((button) => {
      button.onclick = () => void select(button.dataset.settingsDevice);
    });
    restoreSidebarFocus(focusedEntry);
  };
  const select = async (deviceId) => {
    const current = ++generation;
    disposePanel?.();
    disposePanel = null;
    selected = deviceId;
    const route = deviceId === null
      ? { name: "account", page: "settings" }
      : deviceId === "archive"
        ? { name: "account", page: "archive" }
        : { name: "device", id: deviceId };
    markRoute(route);
    paintSidebar();
    content.className = "settings-content";
    content.scrollTop = 0;
    const panel = document.createElement("div");
    content.replaceChildren(panel);
    const options = {
      root: panel,
      deviceId,
      embedded: true,
      registerDispose: (dispose) => { if (generation === current) disposePanel = dispose; else dispose(); },
      isCurrent: () => generation === current,
      onDevicesChanged: () => {
        paintDevicePicker();
        paintSidebar();
      },
      onDeviceDeactivated: () => void select(null),
    };
    try {
      if (deviceId === null) await renderSettings(options);
      else if (deviceId === "archive") await renderArchive(options);
      else await renderDeviceSettings(options);
      if (generation === current) paintSidebar();
    } catch (error) {
      if (generation === current) content.innerHTML = `<p role="alert">${esc(error.message)}</p>`;
    }
  };
  scrim.querySelector("[data-settings-close]").onclick = close;
  scrim.onclick = (event) => {
    if (event.target === scrim) close();
    const link = event.target.closest("a[href]");
    if (!link) return;
    if (link.hash === "#/account/settings") {
      event.preventDefault();
      void select(null);
    } else if (link.hash === "#/account/archive") {
      event.preventDefault();
      void select("archive");
    } else if (link.classList.contains("devsettings")) {
      event.preventDefault();
      const deviceId = decodeURIComponent(link.hash.split("/")[2]);
      void select(deviceId);
    }
  };
  const onKeydown = (event) => {
    const nested = [...document.querySelectorAll(".modal-scrim")].at(-1) || document.querySelector("#scrim.show");
    if (event.key === "Tab") trapFocus(nested || scrim, event);
    if (nested || event.key !== "Escape") return;
    event.stopImmediatePropagation();
    close();
  };
  document.addEventListener("keydown", onKeydown, true);
  const unsubscribe = onDeviceStateChanged(paintSidebar);
  App.viewDispose = () => {
    generation += 1;
    disposePanel?.();
    unsubscribe();
    document.removeEventListener("keydown", onKeydown, true);
    background.inert = wasInert;
    document.body.classList.remove("settings-open");
    scrim.remove();
    if (previousFocus?.isConnected) previousFocus.focus();
  };
  void select(selected);
  scrim.querySelector("[data-settings-close]").focus();
}

function trapFocus(container, event) {
  const controls = [...container.querySelectorAll('button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled])')]
    .filter((node) => !node.hidden && node.getClientRects().length);
  const first = controls[0];
  const last = controls.at(-1);
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last?.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first?.focus();
  } else if (!container.contains(document.activeElement)) {
    event.preventDefault();
    first?.focus();
  }
}
