// Connection gate. Before the app can load we need a live, owned device.
// Three entry states: onboarding (no devices yet), waiting (devices exist but
// none online — auto-reconnect), connected (open the E2EE session and render).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, render } from "../app.js";
import { CONNECTION_STATUS, claimHomeContext, openDeviceSessions, setConn } from "../connection.js";
import { refreshDevices, paintDevicePicker } from "../devices.js";
import { approveDevice, fetchDownloads, lookupDevice, mintInstallCommand } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { startFeed, stopFeed } from "../core/taskFeed.js";
import { startCacheSync } from "../core/cacheSync.js";
import { initInboxRail } from "../core/inboxShell.js";
import { initToolbar } from "../core/toolbar.js";

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

async function enterApp() {
  if (App._connecting) return;
  let home = null;
  App._connecting = true;
  try {
    // Every online device is opened at once; the app comes up on whichever
    // answers first rather than waiting out the slowest one. A sticky device
    // that is online claims home as it lands, so this only names one when
    // nobody has.
    home = await openDeviceSessions().first;
  } finally {
    App._connecting = false;
  }
  claimHomeContext(home);
  if (App._watch) {
    clearInterval(App._watch);
    App._watch = null;
  }
  setGate(false);
  paintDevicePicker();
  setConn(CONNECTION_STATUS.connected);
  // The bridges are already greeted: connection.js greets every device as it
  // lands it, which is before the first one answers here — so the surfaces
  // mount on the cadence each bridge has earned. A bridge that pushes lets them
  // stand down to the safety poll; one that does not leaves every interval
  // exactly where it has always been.
  startFeed();
  startCacheSync();
  initInboxRail();
  initToolbar();
  render(); // the hash route survives the gate, so deep links land where they point
}

// Poll for a device to come online, then connect automatically.
function watchForOnline() {
  if (App._watch) clearInterval(App._watch);
  App._watch = setInterval(async () => {
    const devices = await refreshDevices();
    if (!devices.length) {
      clearInterval(App._watch);
      App._watch = null;
      await renderOnboarding();
      return;
    }
    paintWaiting(devices);
    if (devices.some((d) => d.status === "online")) {
      try {
        await enterApp();
      } catch {
        /* warming up */
      }
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
  setConn(CONNECTION_STATUS.noDevices);
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
  setConn(CONNECTION_STATUS.deviceOffline);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Waiting for your device</h1>
      <p class="settings-intro" style="margin:0 0 18px">None of your devices are online right now. Start your bridge and Build will connect automatically — no need to refresh.</p>
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

export async function boot() {
  setGate(true);
  setConn(CONNECTION_STATUS.connecting);
  const devices = await refreshDevices();
  if (!devices.length) {
    await renderOnboarding();
    return;
  }
  if (devices.some((d) => d.status === "online")) {
    try {
      await enterApp();
      return;
    } catch {
      /* status stale or warming up → waiting */
    }
  }
  renderWaiting(devices);
  watchForOnline();
}
