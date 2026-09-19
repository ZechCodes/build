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
  devicesBlockedText,
  devicesNotReachedYetText,
  esc,
  waitingForDeviceText,
} from "../core/text.js";
import { App, render, unmountView } from "../app.js";
import { connectDevice, openDeviceSessions, securityStopText } from "../connection.js";
import { deviceAwayText, deviceAwayWord } from "../core/deviceAway.js";
import { contextFor, existingDeviceLifecycle, knownContexts, liveContexts, onDeviceStateChanged } from "../core/deviceContexts.js";
import { deviceNameOf } from "../core/devicePolicy.js";
import { markNothingAnswers, paintDevicePicker, readPresence, refreshDevices, stopWatchingPresence, watchPresence } from "../devices.js";
import { renderAppBehindBridgeGate, renderBridgeBehindAppGate } from "./versionGate.js";
import { approveDevice, fetchDownloads, lookupDevice, mintInstallCommand } from "../api.js";
import { currentPlatformKey } from "../core/platform.js";
import { downloadsPlaceholderHtml, mountDownloads } from "../core/downloads.js";
import { openAddDevice } from "../sheets/addDevice.js";
import { startFeed, stopFeed } from "../core/taskFeed.js";
import { startCacheSync } from "../core/cacheSync.js";
import { initInboxRail } from "../core/inboxShell.js";
import { initToolbar } from "../core/toolbar.js";
import { DEVICES_ADDRESS, readCached } from "../core/localCache.js";

/** Whether the cache's two readers are up. They are started once, before any
 *  session answers, and stood down when a gate screen takes the page (which is
 *  what stops the feed). Without this the three-second watch below would stop
 *  and restart both of them on every tick it spends waiting for a machine. */
let cacheReadersUp = false;

// The gate screens are self-contained — body.gated hides the inbox rail (and
// its reopen toggle), the toolbar, the agent rail and the console via CSS while
// they own #root.
function setGate(on) {
  App.gated = on;
  document.body.classList.toggle("gated", on);
  if (on) {
    $("#devpick").hidden = true;
    cacheReadersUp = false;
    stopFeed(); // no session to poll — the inbox is hidden while gated
    // The account's presence is the app's cadence (spec rule 6); a gated page
    // has its own, quicker one below, and two of them would read twice.
    stopWatchingPresence();
  }
}

/** Start reading the cache: the sync layer, and the feed's seed. Neither says
 *  anything on the wire until a context is live. Answers when the feed has
 *  read the disk, so a caller painting a shell can put the rows in with it. */
function startCacheReaders() {
  if (cacheReadersUp) return undefined;
  cacheReadersUp = true;
  startCacheSync();
  return startFeed();
}

/** Whether the reader is standing in the app rather than on a gate screen.
 *  A shell the cache painted is as real as one a session painted, so the gate
 *  screens below never take it away: an account with nothing that can answer
 *  is the mark on the device picker, not a page with the app removed from it.
 *  (Both version gates still take the page — a bridge answering in a shape
 *  this tab cannot read is not something the cache can stand in for.) */
const shellIsPainted = () => App.gated === false;

/** A gate screen with a painted shell under it does not paint. It says what it
 *  would have said on the one control that is about the account's machines,
 *  and answers that the reader keeps their page. */
function keepPaintedShell() {
  if (!shellIsPainted()) return false;
  markNothingAnswers(true);
  return true;
}

/**
 * The whole app, off the disk, before anything is asked of the network.
 *
 * Everything the first screen shows was written by the last session: which
 * machines the account has, and each machine's board with its two lists. So
 * the gate reads them and hands the page straight to the reader — the device
 * GET and the E2EE handshake that follow are catching up, not loading.
 *
 * Answers whether it painted. An empty cache has nothing to stand on, and the
 * gate screens are what a first run sees, exactly as before.
 */
async function paintFromCache() {
  if (shellIsPainted()) return true;
  const devices = (await readCached(DEVICES_ADDRESS))?.value || [];
  if (!devices.length) return false;
  App.devices = devices;
  setGate(false);
  // The rail's rows, read off disk for every machine the list names — awaited,
  // so the shell and what is in it land in the same frame.
  await startCacheReaders();
  paintDevicePicker();
  initInboxRail();
  initToolbar();
  render(); // the hash route survives a reload, so deep links paint from disk too
  return true;
}

// Whether the gate is holding the app for want of a machine that can answer,
// and how it hears that that changed.
let holding = false;
let stopWatchingDevices = null;
// One boot at a time, and a counter every screen paint checks itself against:
// two overlapping boots must not paint over each other.
let gateGeneration = 0;
let connectingPromise = null;

async function connectToApp(asked) {
  // The two readers of the cache come up first, so the inbox has its rows
  // while the handshake is still happening rather than after it.
  startCacheReaders();
  // Every online device is opened at once; the app comes up on whichever
  // answers first rather than waiting out the slowest one. The gate names no
  // home: which device that is, the account list and the user's pick already
  // say, and each device takes it in hand as it lands.
  await openDeviceSessions(asked).first;
  gateGeneration += 1;
  stopWatchingForOnline();
  handBackToReader();
  initInboxRail();
  initToolbar();
  render(); // the hash route survives the gate, so deep links land where they point
  // Last, because it reads the account as it starts listening: the machine that
  // answered may have greeted a bridge no adapter here speaks to while the app
  // was coming up, and the page it has earned is the version gate this stands.
  holdAppWhileNoDeviceAnswers();
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
  markNothingAnswers(false); // something answered; the picker stops saying nothing does
  paintDevicePicker();
  startFeed();
  // Which machines the account has, and which of them are up, is read from the
  // api from here on: a late device joins on it, and a machine whose bridge has
  // gone is marked away on it (spec rule 6).
  watchPresence();
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
 *
 * The account is read as this starts listening as well as whenever it moves: a
 * greeting settles while the app is still coming up, and a bridge no adapter
 * here speaks to announces that once. A gate that only listened would wait for
 * news that had already been told.
 */
export function holdAppWhileNoDeviceAnswers() {
  stopWatchingDevices?.();
  const readAccount = () => {
    if (liveContexts().length) return leaveHold();
    if (gatedContext() && !hasOnlineRecoveryCandidate()) return holdForDevices();
    if (allDevicesOffline()) return holdForDevices();
    // Before the first presence read there is no state to put on screen, but a
    // device landing still owes the shell its one handoff (feed, route, chrome).
    if (!App.devices.length) {
      holding = true;
      return;
    }
    // A pre-connection listener is waiting to perform the shell's one-time
    // handoff when the first context lands. Once contexts exist, losing them
    // while presence remains online is recovery and keeps the shell in place.
    if (!knownContexts().length) {
      holding = true;
      return;
    }
    leaveHold();
  };
  stopWatchingDevices = onDeviceStateChanged(readAccount);
  readAccount();
}

/** Nothing can answer: the mounted view goes, and the screen says why. A
 *  machine whose bridge speaks an API major nothing here claims is answering —
 *  in a shape this tab cannot read — so it gets the version gate rather than
 *  the waiting screen.
 *
 *  The waiting screen carries the account's presence with it. This is the
 *  reader that hears an actually offline machine come back, on the gate's own
 *  three seconds rather than the app's fifteen. Online connection recovery is
 *  owned by connection.js and never takes the mounted workspace away. */
function holdForDevices() {
  if (holding && holdIsOnScreen()) {
    // The screen is up and what the machines say has changed under it: one of
    // them is blocked now, with a reason on its row and a retry to press (rule
    // 3). A version gate has no list to repaint and paints nothing, and a
    // shell that kept the page is already wearing its mark.
    paintWaiting(App.devices);
    return;
  }
  holding = true;
  const behind = gatedContext();
  if (behind) {
    unmountView();
    showVersionGate(behind);
    return;
  }
  renderWaiting(App.devices);
  // Whatever the screen is, the account is watched at the gate's own cadence:
  // a machine coming back is what hands the app — or the wire under a painted
  // shell — back to the reader.
  watchForOnline();
}

const allDevicesOffline = () => App.devices.length > 0 && App.devices.every((device) => device.status === "offline");

/** Whether the screen this hold stands on is still on the page. A hold is only
 *  as good as what it put there: one whose screen has been taken down leaves a
 *  page with nothing on it, so it is stood up again rather than trusted. A
 *  hold over a painted shell put the app itself there, which is still up. */
const holdIsOnScreen = () => Boolean($("#waitlist") || gatedDeviceId || shellIsPainted());

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
async function enterApp(asked) {
  if (connectingPromise) return connectingPromise;
  App._connecting = true;
  connectingPromise = connectToApp(asked);
  try {
    return await connectingPromise;
  } finally {
    connectingPromise = null;
    App._connecting = false;
  }
}

// Poll for a device to come online, then connect automatically. It is the same
// read the app makes on its own cadence (devices.js): a machine the account has
// started calling online again is opened by it, and one whose bridge has gone
// is marked away — which is what lets a machine that was blocked be asked for
// again when it comes back.
function watchForOnline() {
  stopWatchingForOnline();
  const generation = gateGeneration;
  App._watch = setInterval(async () => {
    const devices = await readPresence();
    if (generation !== gateGeneration) return;
    if (!devices) return;
    if (!devices.length) {
      stopWatchingForOnline();
      await renderOnboarding();
      return;
    }
    paintWaiting(devices);
    try {
      await enterApp();
    } catch {
      if (generation !== gateGeneration) return;
      if (!renderFatalConnectionState() && devices.some((device) => device.status === "online")) {
        enterShellWhileRecovering();
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
  const intro = $("#waitintro");
  if (intro) intro.textContent = waitingText(devices);
  const html = devices.map(waitingRowHtml).join("");
  // This runs every three seconds while the page waits for a device. Until one
  // of them says something new, the list is left exactly as it is.
  if (list.innerHTML !== html) list.innerHTML = html;
}

/** One machine under the waiting screen's heading: what the account calls it,
 *  and — for one nothing could reach directly — the reason in a word and the
 *  one thing a reader can do about it (spec rule 3). A machine that is simply
 *  offline has nothing to press: it comes back when its bridge does. */
function waitingRowHtml(device) {
  const context = contextFor(device.id);
  const blocked = Boolean(context?.blocked);
  const word = blocked ? deviceAwayWord(context) : device.status;
  return `
    <div class="projrow"><span class="pname">${esc(device.name)}</span>
      <span class="ppath mono" style="font-size:11px">${esc(device.fingerprint.slice(0, 16))}…</span>
      <span class="dim" style="font-size:11.5px"><span class="dot" style="background:${device.status === "online" && !blocked ? "var(--green)" : "var(--dim)"}"></span> ${esc(word)}</span>
      ${blocked ? `<button class="btn mini" type="button" data-retry-device="${esc(device.id)}">Retry</button>` : ""}</div>`;
}

/** A reader asking one machine again from the screen that is waiting on it. The
 *  hold hands the app back by itself the moment that machine answers; one that
 *  could not be reached again wears its new reason here. */
function retryOneDevice(deviceId) {
  connectDevice(deviceId).catch(() => paintWaiting(App.devices));
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
  blocked: devicesBlockedText,
  unreached: devicesNotReachedYetText,
  lone: (devices) => deviceUnreachableText(devices[0].name, contextFor(devices[0].id)?.offlineSince || Date.now()),
  all: allDevicesOfflineText,
};

const waitingSituation = (devices) => {
  // A machine the account still calls online that nothing here could reach
  // directly is the one situation on this screen with something to press: it is
  // not offline, and waiting will not fix it (rule 3).
  if (devices.some((device) => device.status === "online" && contextFor(device.id)?.blocked)) return "blocked";
  const unreached = devices.some((device) => device.status === "online" && !contextFor(device.id));
  if (unreached) return "unreached";
  return devices.length === 1 ? "lone" : "all";
};

const waitingText = (devices) => WAITING_TEXT[waitingSituation(devices)](devices);

function renderWaiting(devices) {
  if (keepPaintedShell()) return;
  unmountView();
  setGate(true);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">${esc(waitingForDeviceText(devices.length))}</h1>
      <p class="settings-intro" id="waitintro" style="margin:0 0 18px">${esc(waitingText(devices))}</p>
      <div class="panel"><div id="waitlist"></div></div>
      <div class="wait-row"><span class="dim" id="watchmsg">⟳ watching for a device to come online…</span>
        <button class="btn" id="retrybtn">Retry now</button>
        <button class="btn" id="addmore">Add another device…</button></div>
      <div class="adderr" id="oerr"></div>
    </div>`;
  paintWaiting(devices);
  $("#waitlist").onclick = (event) => {
    const asked = event.target.closest("[data-retry-device]");
    if (asked) retryOneDevice(asked.dataset.retryDevice);
  };
  // A machine this client has stopped dialling — its key was not the key this
  // account pinned — is the one thing on this screen that waiting will not fix,
  // so it is said where the screen says what went wrong.
  $("#oerr").textContent = securityStopText();
  // The account-wide Retry: every machine on this screen is one this client
  // could not reach, so it asks for all of them again rather than for whatever
  // a plain boot would think was worth asking (rule 3).
  $("#retrybtn").onclick = () => boot({ retry: true });
  $("#addmore").onclick = () => openAddDevice(boot);
}

function renderConnectionRefusal(message) {
  setGate(true);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Couldn’t connect securely</h1>
      <p class="settings-intro">Build stopped connecting to protect this device.</p>
      <div class="adderr" id="oerr">${esc(message)}</div>
    </div>`;
}

function renderDirectConnectionUnavailable(context) {
  setGate(true);
  const name = deviceNameOf(App.devices, context.deviceId);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Direct connection unavailable</h1>
      <p class="settings-intro">${esc(deviceAwayText(context, name))}</p>
    </div>`;
}

function renderPresenceUnavailable() {
  if (keepPaintedShell()) return watchForBoot();
  setGate(true);
  $("#root").innerHTML = `
    <div style="max-width:680px;margin:44px auto 0;padding:0 16px">
      <h1 style="margin:0 0 6px">Loading your devices…</h1>
      <p class="settings-intro">Build couldn’t refresh device status. Your last known workspace is unchanged.</p>
      <div class="wait-row"><button class="btn" id="retrybtn">Retry now</button></div>
    </div>`;
  $("#retrybtn").onclick = () => boot();
  watchForBoot();
}

/** Ask the whole account again on the gate's cadence, for as long as this boot
 *  is the one the page belongs to. */
function watchForBoot() {
  const generation = gateGeneration;
  App._watch = setInterval(() => {
    if (generation === gateGeneration) boot();
  }, 3000);
}

function enterShellWhileRecovering() {
  handBackToReader();
  startCacheReaders();
  initInboxRail();
  initToolbar();
  render();
  holdAppWhileNoDeviceAnswers();
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

/** The account's machines, or nothing and a page that says so.
 *
 *  A failed read is the one case where what the app holds is all it has: the
 *  statuses are whatever the last successful read left, which after a paint
 *  from the cache is as old as the cache. So the state of the page decides
 *  what that costs. A gate screen with a machine the account last called
 *  online is handed the shell to stand in while the read recovers; anything
 *  else — including a shell the cache already painted — goes to the screen
 *  that says the account could not be read, which over a painted shell is the
 *  mark on the picker and the gate's own watch rather than a page. */
async function readDevicesForBoot(generation) {
  try {
    return await refreshDevices();
  } catch {
    if (generation !== gateGeneration) return null;
    if (App.gated && App.devices.some((device) => device.status === "online")) enterShellWhileRecovering();
    else renderPresenceUnavailable();
    return null;
  }
}

async function finishFailedBoot(generation) {
  if (renderFatalConnectionState()) return;
  const refreshedDevices = await readDevicesForBoot(generation);
  if (!refreshedDevices || generation !== gateGeneration) return;
  if (!refreshedDevices.length) return renderOnboarding();
  if (!allDevicesOffline()) return enterShellWhileRecovering();
  renderWaiting(refreshedDevices);
  watchForOnline();
}

function renderFatalConnectionState() {
  const refusal = securityStopText();
  if (refusal) {
    if (hasOnlineRecoveryCandidate()) return false;
    renderConnectionRefusal(refusal);
    return true;
  }
  if (gatedContext()) {
    if (hasOnlineRecoveryCandidate()) return false;
    holdForDevices();
    return true;
  }
  const unsupportedTransport = knownContexts().find((context) => context.blocked === "no-webrtc");
  if (!unsupportedTransport) return false;
  if (hasOnlineRecoveryCandidate()) return false;
  renderDirectConnectionUnavailable(unsupportedTransport);
  return true;
}

function hasOnlineRecoveryCandidate() {
  const barred = new Set(
    knownContexts()
      .filter((context) =>
        context.unsupported ||
        context.blocked === "no-webrtc" ||
        existingDeviceLifecycle(context.deviceId)?.snapshot().securityStop)
      .map((context) => context.deviceId),
  );
  return App.devices.some((device) => device.status === "online" && !barred.has(device.id));
}

export async function boot({ retry = false } = {}) {
  const generation = ++gateGeneration;
  stopWatchingForOnline();
  // Before the network: the page the reader had is the page they get back.
  await paintFromCache();
  if (generation !== gateGeneration) return;
  const devices = await readDevicesForBoot(generation);
  if (!devices) return;
  if (generation !== gateGeneration) return;
  if (!devices.length) {
    await renderOnboarding();
    return;
  }
  try {
    await enterApp({ retry });
    return;
  } catch {
    /* API presence is only a hint; the relay is not ready yet → waiting */
  }
  if (generation !== gateGeneration) return;
  await finishFailedBoot(generation);
}
