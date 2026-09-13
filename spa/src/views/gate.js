// Connection gate. Before the app can load we need a live, owned device.
// Three entry states: onboarding (no devices yet), waiting (devices exist but
// none online — auto-reconnect), connected (open the E2EE session and render).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, render } from "../app.js";
import { openBootSession, adoptSession, greetLiveBridge, onBridgeSelected, setConn } from "../connection.js";
import { deviceName, refreshDevices, paintDevicePicker } from "../devices.js";
import { renderAppBehindBridgeGate, renderBridgeBehindAppGate } from "./versionGate.js";
import { approveDevice, fetchDownloads, lookupDevice, mintInstallCommand } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { startFeed, stopFeed } from "../core/taskFeed.js";
import { startCacheSync } from "../core/cacheSync.js";
import { initInboxRail } from "../core/inboxShell.js";
import { initToolbar } from "../core/toolbar.js";

let gateGeneration = 0;
let connectingPromise = null;

// The gate screens are self-contained — body.gated hides the inbox rail (and
// its reopen toggle), the toolbar, the agent rail and the console via CSS while
// they own #root.
function setGate(on) {
  App.gated = on;
  document.body.classList.toggle("gated", on);
  if (on) {
    $("#devpick").hidden = true;
    stopFeed(); // no session to poll — the inbox is hidden while gated
  }
}

async function connectToApp() {
  // Prefer the remembered device while it reports online, then try the other
  // known devices in bootstrap order.
  adoptSession(await openBootSession(App.devices));
  gateGeneration += 1;
  if (App._watch) {
    clearInterval(App._watch);
    App._watch = null;
  }
  setGate(false);
  paintDevicePicker();
  setConn('<span class="dot"></span>connected');
  // Before the surfaces mount, so they take the cadence this bridge earns: a
  // bridge that pushes lets them stand down to the safety poll, and one that
  // does not leaves every interval exactly where it has always been.
  greetLiveBridge();
  startFeed();
  startCacheSync();
  initInboxRail();
  initToolbar();
  render(); // the hash route survives the gate, so deep links land where they point
}

async function enterApp() {
  if (connectingPromise) return connectingPromise;
  App._connecting = true;
  connectingPromise = connectToApp();
  try {
    return await connectingPromise;
  } finally {
    connectingPromise = null;
    App._connecting = false;
  }
}

// Poll for a device to come online, then connect automatically.
function watchForOnline() {
  if (App._watch) clearInterval(App._watch);
  const generation = gateGeneration;
  App._watch = setInterval(async () => {
    const devices = await refreshDevices();
    if (generation !== gateGeneration) return;
    if (!devices.length) {
      clearInterval(App._watch);
      App._watch = null;
      await renderOnboarding();
      return;
    }
    paintWaiting(devices);
    try {
      await enterApp();
    } catch {
      /* warming up */
    }
  }, 3000);
}

const STEP_BULLET =
  "flex:none;width:22px;height:22px;border-radius:50%;background:var(--accent-soft);color:var(--accent);display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600";

const stepHtml = (number, body) =>
  `<div style="display:flex;gap:12px;align-items:flex-start;padding:10px 0"><span style="${STEP_BULLET}">${number}</span><div>${body}</div></div>`;

/** The three steps of a first run, as pure html. Step 1 holds the downloads
 *  placeholder; mountDownloads fills it from the api once it answers. */
function onboardingStepsHtml() {
  return `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Welcome to Build</h1>
      <p class="settings-intro" style="margin:0 0 10px">Your coding agents run on your own devices, end-to-end encrypted. Add a device to begin — only paired devices can read your tasks, plans, and diffs.</p>
      <p class="dim" style="font-size:12.5px;margin:0 0 22px">Agents run on your machine in YOLO mode; Build makes what they did visible, it does not sandbox them.</p>
      <div class="panel">
        ${stepHtml(
          1,
          `<b>Install the bridge</b> on the machine where your code lives.
          <div style="margin-top:8px">${downloadsPlaceholderHtml()}</div>`,
        )}
        ${stepHtml(
          2,
          `<b>Enter its pairing code</b> — the bridge prints it when it starts, and again while it waits for you.
          <div class="addproj" style="margin-top:6px"><input id="ocode" placeholder="e.g. G6ZP-KD2U" style="text-transform:uppercase" autofocus />
            <button class="btn primary" id="olookup">Add device</button></div>`,
        )}
        ${stepHtml(3, "<b>Compare the fingerprint</b> with what the bridge printed, then approve.")}
      </div>
      <div id="opairbox"></div>
      <div class="adderr" id="oerr"></div>
    </div>`;
}

/** Lookup → fingerprint → approve, unchanged. The pairing code is what actually
 *  pairs a device, so it is wired before the downloads block is asked for and
 *  stays usable whatever that route answers. */
function bindPairing() {
  const lookup = async () => {
    const code = $("#ocode").value.trim().toUpperCase();
    if (!code) return;
    $("#oerr").textContent = "";
    let device;
    try {
      device = await lookupDevice(code);
    } catch (e) {
      $("#oerr").textContent = e.message;
      return;
    }
    $("#opairbox").innerHTML = `
      <div class="panel" style="margin-top:14px">
        <div class="row"><span class="k">Device</span><span class="v">${esc(device.name)}</span></div>
        <div class="row"><span class="k">Fingerprint</span><span class="v mono" style="font-size:11px;word-break:break-all">${esc(device.fingerprint)}</span></div>
        <div class="dim" style="font-size:12px;margin:8px 0">Confirm this matches what the bridge printed, then approve.</div>
        <button class="btn primary" id="oapprove">Approve &amp; pair</button></div>`;
    $("#oapprove").onclick = async () => {
      $("#oapprove").disabled = true;
      $("#oerr").textContent = "";
      try {
        await approveDevice(code);
        $("#opairbox").innerHTML = "";
        await boot();
      } catch (e) {
        $("#oerr").textContent = e.message;
        $("#oapprove").disabled = false;
      }
    };
  };
  $("#olookup").onclick = lookup;
  $("#ocode")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") lookup();
  });
}

// The first run: no approved device on this account. Nobody can enter a pairing
// code before there is a bridge to print one, so the download comes first — on
// the same screen, above the code the bridge will print.
async function renderOnboarding() {
  setGate(true);
  setConn('<span class="dot" style="background:var(--dim)"></span>no devices');
  $("#root").innerHTML = onboardingStepsHtml();
  bindPairing();
  await mountDownloads($("#root"), {
    fetchDownloads,
    mintInstallCommand,
    platformKey: currentPlatformKey(),
    clipboard: navigator.clipboard,
  });
}

function paintWaiting(devices) {
  const list = $("#waitlist");
  if (!list) return;
  const intro = $("#waitintro");
  if (intro) {
    intro.textContent = devices.some((device) => device.status === "online")
      ? "Your devices report online, but Build could not reach one yet. It will keep trying automatically — no need to refresh."
      : "None of your devices are online right now. Start your bridge and Build will connect automatically — no need to refresh.";
  }
  const html = devices
    .map(
      (d) => `
    <div class="projrow"><span class="pname">${esc(d.name)}</span>
      <span class="ppath mono" style="font-size:11px">${esc(d.fingerprint.slice(0, 16))}…</span>
      <span class="dim" style="font-size:11.5px"><span class="dot" style="background:${d.status === "online" ? "var(--green)" : "var(--dim)"}"></span> ${esc(d.status)}</span></div>`,
    )
    .join("");
  // This runs every three seconds while the page waits for a device. Until one
  // of them says something new, the list is left exactly as it is.
  if (list.innerHTML !== html) list.innerHTML = html;
}

function renderWaiting(devices) {
  setGate(true);
  setConn('<span class="dot" style="background:var(--amber)"></span>device offline');
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Waiting for your device</h1>
      <p class="settings-intro" id="waitintro" style="margin:0 0 18px"></p>
      <div class="panel"><div id="waitlist"></div></div>
      <div class="row" style="margin-top:14px"><span class="dim" id="watchmsg">⟳ watching for a device to come online…</span>
        <button class="btn" id="retrybtn" style="margin-left:auto">Retry now</button>
        <button class="btn" id="addmore">Add another device…</button></div>
      <div class="adderr" id="oerr"></div>
    </div>`;
  paintWaiting(devices);
  $("#retrybtn").onclick = () => boot();
  $("#addmore").onclick = () => openAddDevice(boot);
}

// ---- the version gates (wire spec step 2.5) ----------------------------------
//
// Every greeting selects an adapter for the bridge that answered it, or names
// the side that is out of date. The two screens below own #root for as long
// as that is the answer; the greeting of a reconnect onto a bridge an adapter
// claims — the user updated it, or switched device — lets the app back in.

let versionGated = false;

const gatedDeviceName = () => deviceName(App.session?.deviceId);

/** The bridge speaks a newer major than this bundle. The reload is offered
 *  only once the served-version watcher has found something newer to land on. */
function showAppBehindGate(bridgeVersion) {
  renderAppBehindBridgeGate($("#root"), {
    deviceName: gatedDeviceName(),
    bridgeVersion,
    onReload: App.updateAvailable ? () => location.reload() : null,
  });
}

/** The bridge speaks an older major than any adapter here. The screen stands
 *  before the install line is minted, and carries it once it is. */
async function showBridgeBehindGate(bridgeVersion) {
  const root = $("#root");
  const shown = { deviceName: gatedDeviceName(), bridgeVersion };
  renderBridgeBehindAppGate(root, shown);
  let minted;
  try {
    minted = await mintInstallCommand();
  } catch {
    return; // the instruction stands without the line
  }
  if (versionGated && root === $("#root")) {
    renderBridgeBehindAppGate(root, { ...shown, installCommand: minted.install_command });
  }
}

/** The surfaces were mounted before the greeting gated them; the feed is the
 *  one thing the gate stopped, and the route is still where it pointed. */
function leaveVersionGate() {
  versionGated = false;
  setGate(false);
  paintDevicePicker();
  startFeed();
  render();
}

function onAdapterSelected(selection) {
  if (!selection.unsupported) {
    if (versionGated) leaveVersionGate();
    return;
  }
  versionGated = true;
  setGate(true);
  if (selection.unsupported === "app") showAppBehindGate(selection.version);
  else showBridgeBehindGate(selection.version);
}

onBridgeSelected(onAdapterSelected);

export async function boot() {
  const generation = ++gateGeneration;
  if (App._watch) {
    clearInterval(App._watch);
    App._watch = null;
  }
  setGate(true);
  setConn('<span class="dot" style="background:var(--amber)"></span>connecting…');
  const devices = await refreshDevices();
  if (generation !== gateGeneration) return;
  if (!devices.length) {
    await renderOnboarding();
    return;
  }
  try {
    await enterApp();
    return;
  } catch {
    /* API presence is only a hint; the relay is not ready yet → waiting */
  }
  if (generation !== gateGeneration) return;
  const refreshedDevices = await refreshDevices();
  if (generation !== gateGeneration) return;
  renderWaiting(refreshedDevices);
  watchForOnline();
}
