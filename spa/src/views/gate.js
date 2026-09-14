// Connection gate. Before the app can load we need a live, owned device.
// Three entry states: onboarding (no devices yet), waiting (devices exist but
// none online — auto-reconnect), connected (open the E2EE session and render).
//
// The waiting screen is not only a boot state. Every surface in the app is
// about a machine, so an account that has run out of machines that can answer
// has nothing to stand on: the gate takes the app back and says which machines
// it is waiting for, and hands it straight back when one of them lands.

import { $ } from "../dom.js";
import { allDevicesOfflineText, deviceUnreachableText, esc, waitingForDeviceText } from "../core/text.js";
import { App, render, unmountView } from "../app.js";
import { openDeviceSessions } from "../connection.js";
import { knownContexts, liveContexts, onDeviceStateChanged } from "../core/deviceContexts.js";
import { deviceNameOf } from "../core/devicePolicy.js";
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

// Whether the gate is holding the app for want of a machine that can answer,
// and how it hears that that changed.
let holding = false;
let stopWatchingDevices = null;

async function enterApp() {
  if (App._connecting) return;
  App._connecting = true;
  try {
    // Every online device is opened at once; the app comes up on whichever
    // answers first rather than waiting out the slowest one. The gate names no
    // home: which device that is, the account list and the user's pick already
    // say, and each device takes it in hand as it lands.
    await openDeviceSessions().first;
  } finally {
    App._connecting = false;
  }
  stopWatchingForOnline();
  holdAppWhileNoDeviceAnswers();
  handBackToReader();
  startCacheSync();
  initInboxRail();
  initToolbar();
  render(); // the hash route survives the gate, so deep links land where they point
}

/** Give the page back to the reader: the shell is theirs again, the picker says
 *  what the rail is showing, and the devices are read on their cadence.
 *
 *  The bridges are already greeted: connection.js greets every device as it
 *  lands it, which is before the first one answers here — so the surfaces mount
 *  on the cadence each bridge has earned. A bridge that pushes lets them stand
 *  down to the safety poll; one that does not leaves every interval exactly
 *  where it has always been. */
function handBackToReader() {
  holding = false;
  setGate(false);
  paintDevicePicker();
  startFeed();
}

/**
 * Hold the app whenever nothing can answer, and hand it back when something
 * can.
 *
 * Reachability is not news the feed carries — a row does not change when the
 * machine behind it goes — so it is heard from the registry. One device of
 * several going is the rail's business: its rows grey and the account carries
 * on. The last one going is the whole app's, because there is no longer a
 * machine for any surface to be about.
 */
export function holdAppWhileNoDeviceAnswers() {
  stopWatchingDevices?.();
  stopWatchingDevices = onDeviceStateChanged(() => (liveContexts().length ? leaveHold() : holdForDevices()));
}

/** Nothing can answer: the mounted view goes, and the waiting screen says which
 *  machines the account is waiting for. Nothing is started to watch for one —
 *  every device is already being asked for on its own backoff, and the first to
 *  land hands the app straight back. */
function holdForDevices() {
  if (holding) return;
  holding = true;
  unmountView();
  renderWaiting(App.devices);
}

/** A machine answered: the reader gets the route they were standing on back,
 *  with the feed reading that machine again. The hold stopped the feed, so this
 *  starts it — the device that just landed is live by the time this runs, and
 *  its own joinFeed finds it already polling. */
function leaveHold() {
  if (!holding) return;
  stopWatchingForOnline();
  handBackToReader();
  render();
}

/** Stop the waiting screen's poll. Every way back into the app runs this: a
 *  poll left armed fires at an app the reader is already standing in, enters it
 *  again, and builds the rail, the toolbar and the route over whatever was
 *  mounted. */
function stopWatchingForOnline() {
  clearInterval(App._watch);
  App._watch = null;
}

// Poll for a device to come online, then connect automatically.
function watchForOnline() {
  stopWatchingForOnline();
  App._watch = setInterval(async () => {
    const devices = await refreshDevices();
    if (!devices.length) {
      stopWatchingForOnline();
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

/** Why the page is waiting, in the account's own words: one machine this client
 *  had and lost is named, with when it went unreachable; several of them — or
 *  none this client ever reached — is a sentence about the account. */
function waitingText() {
  const contexts = knownContexts();
  if (contexts.length !== 1) return allDevicesOfflineText();
  const [context] = contexts;
  return deviceUnreachableText(deviceNameOf(App.devices, context.deviceId), context.offlineSince || Date.now());
}

function renderWaiting(devices) {
  setGate(true);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">${esc(waitingForDeviceText(devices.length))}</h1>
      <p class="settings-intro" style="margin:0 0 18px" id="waitnote">${esc(waitingText())} Start your bridge and Build will connect automatically — no need to refresh.</p>
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
