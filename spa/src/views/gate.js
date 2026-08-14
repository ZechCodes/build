// Connection gate. Before the app can load we need a live, owned device.
// Three entry states: onboarding (no devices yet), waiting (devices exist but
// none online — auto-reconnect), connected (open the E2EE session and render).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { RELAY_URL } from "../config.js";
import { onlineStickyDeviceId } from "../core/devicePolicy.js";
import { App, render } from "../app.js";
import { openAppSession, adoptSession, setConn } from "../connection.js";
import { refreshDevices, paintDevicePicker } from "../devices.js";
import { lookupDevice, approveDevice } from "../api.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { startFeed, stopFeed } from "../core/taskFeed.js";
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
  App._connecting = true;
  try {
    // Honor an explicit device choice when that device is online; otherwise
    // whichever of the user's devices answers first.
    const preferDeviceId = onlineStickyDeviceId(App.devices, App.selectedDeviceId);
    adoptSession(await openAppSession({ preferDeviceId }));
  } finally {
    App._connecting = false;
  }
  if (App._watch) {
    clearInterval(App._watch);
    App._watch = null;
  }
  setGate(false);
  paintDevicePicker();
  setConn('<span class="dot"></span>connected');
  startFeed();
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
      renderOnboarding();
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

function renderOnboarding() {
  setGate(true);
  setConn('<span class="dot" style="background:var(--dim)"></span>no devices');
  const api = location.origin;
  const cmd = `BRIDGE_API_URL=${api} BRIDGE_WEB_URL=${api} BRIDGE_RELAY_URL=${RELAY_URL} BRIDGE_IDENTITY_FILE=/tmp/bld/my-device.json BRIDGE_REPO=/tmp/bld/repo BRIDGE_WORKTREES=/tmp/bld/wt build-bridge serve`;
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Welcome to Build</h1>
      <p class="settings-intro" style="margin:0 0 22px">Your coding agents run on your own devices, end-to-end encrypted. Add a device to begin — only paired devices can read your tasks, plans, and diffs.</p>
      <div class="panel">
        <div style="display:flex;gap:12px;align-items:flex-start;padding:10px 0"><span style="flex:none;width:22px;height:22px;border-radius:50%;background:var(--accent-soft);color:var(--accent);display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600">1</span><div><b>Start a bridge</b> on the machine where your code lives.
          <div style="display:flex;gap:8px;align-items:flex-start;margin-top:6px">
            <code class="mono" style="flex:1;font-size:11px;line-height:1.5;padding:8px 10px;overflow-x:auto;white-space:pre-wrap;word-break:break-all">${esc(cmd)}</code>
            <button class="btn mini" id="copycmd">Copy</button></div></div></div>
        <div style="display:flex;gap:12px;align-items:flex-start;padding:10px 0"><span style="flex:none;width:22px;height:22px;border-radius:50%;background:var(--accent-soft);color:var(--accent);display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600">2</span><div><b>Enter its pairing code</b> — the bridge prints it on startup.
          <div class="addproj" style="margin-top:6px"><input id="ocode" placeholder="e.g. G6ZP-KD2U" style="text-transform:uppercase" autofocus />
            <button class="btn primary" id="olookup">Add device</button></div></div></div>
        <div style="display:flex;gap:12px;align-items:flex-start;padding:10px 0"><span style="flex:none;width:22px;height:22px;border-radius:50%;background:var(--accent-soft);color:var(--accent);display:inline-flex;align-items:center;justify-content:center;font-size:12px;font-weight:600">3</span><div><b>Compare the fingerprint</b> with what the bridge printed, then approve.</div></div>
      </div>
      <div id="opairbox"></div>
      <div class="adderr" id="oerr"></div>
    </div>`;
  $("#copycmd").onclick = () => {
    navigator.clipboard?.writeText(cmd);
    $("#copycmd").textContent = "Copied";
    setTimeout(() => ($("#copycmd").textContent = "Copy"), 1500);
  };
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

function paintWaiting(devices) {
  const list = $("#waitlist");
  if (!list) return;
  list.innerHTML = devices
    .map(
      (d) => `
    <div class="projrow"><span class="pname">${esc(d.name)}</span>
      <span class="ppath mono" style="font-size:11px">${esc(d.fingerprint.slice(0, 16))}…</span>
      <span class="dim" style="font-size:11.5px"><span class="dot" style="background:${d.status === "online" ? "var(--green)" : "var(--dim)"}"></span> ${esc(d.status)}</span></div>`,
    )
    .join("");
}

function renderWaiting(devices) {
  setGate(true);
  setConn('<span class="dot" style="background:var(--amber)"></span>device offline');
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
  setConn('<span class="dot" style="background:var(--amber)"></span>connecting…');
  const devices = await refreshDevices();
  if (!devices.length) {
    renderOnboarding();
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
