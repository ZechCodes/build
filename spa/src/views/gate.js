// Connection gate. Before the app can load we need a live, owned device.
// Three entry states: onboarding (no devices yet), waiting (devices exist but
// none online — auto-reconnect), connected (open the E2EE session and render).
//
// The waiting screen is not only a boot state. Every surface in the app is
// about a machine, so an account that has run out of machines that can answer
// has nothing to stand on: the gate takes the app back and says which machines
// it is waiting for, and hands it straight back when one of them lands.

import { $ } from "../dom.js";
import {
  allDevicesOfflineText,
  deviceUnreachableText,
  devicesNotReachedYetText,
  esc,
  waitingForDeviceText,
} from "../core/text.js";
import { App, render, unmountView } from "../app.js";
import { openDeviceSessions, securityStopText } from "../connection.js";
import { contextFor, knownContexts, liveContexts, onDeviceStateChanged } from "../core/deviceContexts.js";
import { deviceNameOf } from "../core/devicePolicy.js";
import { refreshDevices, paintDevicePicker } from "../devices.js";
import { renderAppBehindBridgeGate, renderBridgeBehindAppGate } from "./versionGate.js";
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
// One boot at a time, and a counter every screen paint checks itself against:
// two overlapping boots must not paint over each other.
let gateGeneration = 0;
let connectingPromise = null;

async function connectToApp() {
  // Every online device is opened at once; the app comes up on whichever
  // answers first rather than waiting out the slowest one. The gate names no
  // home: which device that is, the account list and the user's pick already
  // say, and each device takes it in hand as it lands.
  await openDeviceSessions().first;
  gateGeneration += 1;
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
  gatedDeviceId = null;
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

/** Nothing can answer: the mounted view goes, and the screen says why. A
 *  machine whose bridge speaks an API major nothing here claims is answering —
 *  in a shape this tab cannot read — so it gets the version gate rather than
 *  the waiting screen. Otherwise nothing is started to watch for a device:
 *  every one of them is already being asked for on its own backoff, and the
 *  first to land hands the app straight back. */
function holdForDevices() {
  if (holding) return;
  holding = true;
  unmountView();
  const behind = gatedContext();
  if (behind) showVersionGate(behind);
  else renderWaiting(App.devices);
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

/** One boot at a time: a second call while the first is still opening devices
 *  waits on the same promise rather than starting a second handshake. */
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
  stopWatchingForOnline();
  const generation = gateGeneration;
  App._watch = setInterval(async () => {
    const devices = await refreshDevices();
    if (generation !== gateGeneration) return;
    if (!devices.length) {
      stopWatchingForOnline();
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
  if (intro) intro.textContent = waitingText(devices);
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

/** Why the page is waiting, one sentence per situation. A machine the account
 *  calls online that this client has never opened is a handshake still to
 *  happen, so nothing has gone; once every such machine has been reached and
 *  lost, the account has run out — named when there is one machine to name,
 *  and said of all of them when there are several.
 *
 *  It is the account's own list that decides which of those it is, not what
 *  this client happens to hold. A machine that was already down when the page
 *  loaded was never opened here and has no context, so counting contexts read
 *  an account of two machines as an account of one — and named whichever of
 *  them this client had reached. */
const WAITING_TEXT = {
  unreached: devicesNotReachedYetText,
  lone: (devices) => deviceUnreachableText(devices[0].name, contextFor(devices[0].id)?.offlineSince || Date.now()),
  all: allDevicesOfflineText,
};

const waitingSituation = (devices) => {
  const unreached = devices.some((device) => device.status === "online" && !contextFor(device.id));
  if (unreached) return "unreached";
  return devices.length === 1 ? "lone" : "all";
};

const waitingText = (devices) => WAITING_TEXT[waitingSituation(devices)](devices);

function renderWaiting(devices) {
  setGate(true);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">${esc(waitingForDeviceText(devices.length))}</h1>
      <p class="settings-intro" id="waitintro" style="margin:0 0 18px">${esc(waitingText(devices))}</p>
      <div class="panel"><div id="waitlist"></div></div>
      <div class="row" style="margin-top:14px"><span class="dim" id="watchmsg">⟳ watching for a device to come online…</span>
        <button class="btn" id="retrybtn" style="margin-left:auto">Retry now</button>
        <button class="btn" id="addmore">Add another device…</button></div>
      <div class="adderr" id="oerr"></div>
    </div>`;
  paintWaiting(devices);
  // A machine this client has stopped dialling — its key was not the key this
  // account pinned — is the one thing on this screen that waiting will not fix,
  // so it is said where the screen says what went wrong.
  $("#oerr").textContent = securityStopText();
  $("#retrybtn").onclick = () => boot();
  $("#addmore").onclick = () => openAddDevice(boot);
}

// ---- the version gates (wire spec step 2.5) ----------------------------------
//
// Every greeting selects an adapter for the bridge that answered it, or names
// the side that is out of date. The two screens below own #root for as long
// as that is the answer; the greeting of a reconnect onto a bridge an adapter
// claims — the user updated it, or switched device — lets the app back in.

/** The machine a version gate is standing over, while one is up. */
let gatedDeviceId = null;

/** Which screen a machine no adapter here speaks to gets, by the side that is
 *  behind. A kind, so a table rather than a chain. */
const VERSION_GATES = {
  app: showAppBehindGate,
  bridge: showBridgeBehindGate,
};

/** The machine the gate is about when nothing can answer and some machine is
 *  behind: app-behind first, because a reload fixes that one at no cost. */
function gatedContext() {
  const behind = knownContexts().filter((context) => context.unsupported);
  return behind.find((context) => context.unsupported === "app") || behind[0] || null;
}

function showVersionGate(context) {
  gatedDeviceId = context.deviceId;
  setGate(true);
  VERSION_GATES[context.unsupported](context);
}

/** The bridge speaks a newer major than this bundle. The reload is offered
 *  only once the served-version watcher has found something newer to land on. */
function showAppBehindGate(context) {
  renderAppBehindBridgeGate($("#root"), {
    deviceName: deviceNameOf(App.devices, context.deviceId),
    bridgeVersion: context.apiVersion,
    onReload: App.updateAvailable ? () => location.reload() : null,
  });
}

/** The bridge speaks an older major than any adapter here. The screen stands
 *  before the install line is minted, and carries it once it is. */
async function showBridgeBehindGate(context) {
  const root = $("#root");
  const shown = { deviceName: deviceNameOf(App.devices, context.deviceId), bridgeVersion: context.apiVersion };
  renderBridgeBehindAppGate(root, shown);
  let minted;
  try {
    minted = await mintInstallCommand();
  } catch {
    return; // the instruction stands without the line
  }
  if (gatedDeviceId === context.deviceId && root === $("#root")) {
    renderBridgeBehindAppGate(root, { ...shown, installCommand: minted.install_command });
  }
}

export async function boot() {
  const generation = ++gateGeneration;
  if (App._watch) {
    clearInterval(App._watch);
    App._watch = null;
  }
  setGate(true);
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
