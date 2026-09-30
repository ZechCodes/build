// The compose box's wiring: the affordance at the rail's top, the `c` that
// opens it from anywhere, what happens to the text on submit, and the manual
// panel for when the destination is already known.
//
// The model — the queue, the shortcut guard, the markup, the row a capture
// reads as — is core/compose.js. What lives here is the state a box has while
// it is open, and the calls it makes.
//
// Submit is capture-first: the text goes to the daemon, which puts a router on
// it. A device that cannot take it holds it here instead, and the next live
// session sends it. The text is never the thing that is lost.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { canAnswer, creationDevice, deviceFeedView, homeContext } from "./deviceContexts.js";
import { deviceCall, deviceCatalog } from "./inboxDevices.js";
import { deviceNameOf } from "./devicePolicy.js";
import { UNASKED_CATALOG } from "./modelCatalog.js";
import { agentDefaultsFor, agentDefaultsIn, loadAgentDefaults } from "./agentDefaults.js";
import { isConfirmOpen } from "./confirm.js";
import { agentChoiceParams, agentChoicePanelHtml, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import {
  branchOptions,
  captureRow,
  composeBoxHtml,
  composeManualAwayNote,
  composeOfflineNote,
  composePlaceholder,
  composePromptHtml,
  composeShortcutFires,
  flushCaptureQueue,
  loadCaptureQueue,
  manualRoute,
  manualRouteDestination,
  queuedCapture,
  queuedCaptureRow,
  routedCaptureExpired,
  saveCaptureQueue,
  withoutQueued,
} from "./compose.js";
import { replyOrNothing } from "./session.js";
import { esc, messageOf } from "./text.js";
import { captureRecordAddress } from "./captureRecords.js";
import { deleteCached, readCached, subscribeCache, writeCached } from "./localCache.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import "../styles/shell.css";
import { fieldTraits } from "./fieldTraits.js";
import { composeGitOfferHtml, refusedForNoGit } from "./gitInitializationOffer.js";

const CHOICE_PREFIX = "compose-choice";

let queue = []; // captures this client is holding for an absent device
const tracked = new Map(); // capture id → cache-backed row and its settling state
const listeners = new Set(); // who repaints when the held captures change
// The home device's slice of the feed, never the merge: a capture goes to the
// device creation goes to, so the destinations this box offers and the project
// names it prints are that device's. Every device mints a `proj-1`.
let feed = { items: [], projects: [] };
let box = null; // the open box's state, or null while it is shut
let mounted = false;
const boxSnapshot = () => ({
  value: box.value,
  branch: box.branch,
  projectId: box.projectId,
  advancedOpen: box.advancedOpen,
  choiceOpen: box.choiceOpen,
  choice: box.choice,
});
const saveBox = (debounced = false) => {
  if (!box?.state) return;
  if (debounced) box.state.schedule(boxSnapshot());
  else void box.state.write(boxSnapshot());
};
// Every call this box makes is the creation device's, asked for the way every
// other surface asks: by the device it is about, with naming none meaning home.
// A machine that cannot answer hands back a caller that refuses, so nothing
// here asks whether a device is there — only canSend, which is the same
// question the note and the queue are the answer to.
const homeCall = (method, params) => deviceCall(null)(method, params);
const canSend = () => canAnswer(homeContext());
/** What the account calls the machine this box sends to, while it can name it:
 *  the creation device, online or away. */
const creationDeviceName = () => deviceNameOf(App.devices, captureDeviceId());
const projectNameOf = (projectId) =>
  (feed.projects.find((project) => project.id === projectId) || {}).name || projectId || "";
/** The machine a capture this client holds is on: the one creation goes to,
 *  which is the one it was sent to or is waiting for. The rail is one list
 *  across every machine, so a row the feed does not carry yet still has to say
 *  whose it is. */
const captureDeviceId = () => creationDevice();

/** One capture this client is holding, as a row: the daemon's record, the
 *  project name off the creation device's slice of the feed, and the machine
 *  the row is on — which is the one it went to, not whichever is home by the
 *  time some later answer corrects it. */
function heldCaptureRow(capture, deviceId = captureDeviceId()) {
  const projectName = capture.routing ? projectNameOf(capture.routing.project_id) : "";
  return captureRow(capture, { projectName, deviceId });
}

// ---- what the client is holding ----------------------------------------------

/** The capture rows this client owns: the ones it is holding for an absent
 *  device, and the ones it has sent and is still watching. The inbox merges
 *  these with the feed's own, which win on a tie. */
export function pendingCaptureRows(nowMs = Date.now()) {
  for (const [id, entry] of tracked) {
    if (routedCaptureExpired({ state: entry.row?.state, settledAt: entry.settledAt }, nowMs)) {
      entry.unwatch();
      tracked.delete(id);
    }
  }
  return [...queue.map(queuedCaptureRow), ...[...tracked.values()].map((entry) => entry.row).filter(Boolean)];
}

function announce() {
  listeners.forEach((listener) => listener());
}

/**
 * Take a capture record the client itself just changed.
 *
 * A reroute answers with the capture as it now stands, and for one whose route
 * had already settled that answer is the only correction there will ever be:
 * the feed stopped carrying it, so nothing would take the row off its old
 * destination. A settled row restarts its two minutes on screen — the user just
 * acted on it, and the window is there to let them see and undo what they did.
 * One the feed still carries keeps the feed as its record.
 */
const captureRecordDevice = (held, namedDeviceId) => held?.deviceId || namedDeviceId || captureDeviceId();

export async function adoptCaptureRecord(capture, deviceId = null) {
  if (!capture?.id) return;
  const held = tracked.get(capture.id);
  if (held) {
    held.settledAt = held.settledAt ? Date.now() : null;
    held.settling = false;
  }
  await writeCached(captureRecordAddress(captureRecordDevice(held, deviceId), capture.id), capture);
  if (held) await readTracked(capture.id);
}

export function forgetCaptureRecord(captureId) {
  const held = tracked.get(captureId);
  if (!held) return;
  held.unwatch();
  tracked.delete(captureId);
  void deleteCached([captureRecordAddress(held.deviceId, captureId)]);
  announce();
}

/** A tracked row is a projection of the committed capture record. Serialize
 *  reads because an earlier announcement can finish after a later write. */
function readTracked(captureId) {
  const entry = tracked.get(captureId);
  if (!entry) return Promise.resolve();
  const read = async () => {
    const held = await readCached(captureRecordAddress(entry.deviceId, captureId));
    if (tracked.get(captureId) !== entry) return;
    if (!entry.inFeed) entry.row = held ? heldCaptureRow(held.value, entry.deviceId) : null;
    announce();
  };
  entry.read = (entry.read || Promise.resolve()).then(read, read);
  return entry.read;
}

/** Repaint when the held captures change. Returns unsubscribe. */
export function subscribePendingCaptures(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function hold(text) {
  queue = saveCaptureQueue([
    ...queue,
    queuedCapture(text, {
      id: `local-${Date.now()}-${queue.length}`,
      createdAt: new Date().toISOString(),
      deviceId: captureDeviceId(),
    }),
  ]);
  announce();
}

async function track(capture, deviceId) {
  const entry = { row: null, deviceId, settledAt: null, settling: false, inFeed: false, read: null, revision: 0, unwatch: () => {} };
  tracked.set(capture.id, entry);
  const address = captureRecordAddress(deviceId, capture.id);
  entry.unwatch = subscribeCache(address, () => {
    entry.revision += 1;
    void readTracked(capture.id);
  });
  await writeCached(address, capture);
  await readTracked(capture.id);
}

/**
 * Send what the client is holding, oldest first. Captures go to the machine
 * creation goes to, so this is called whenever that machine has a live session
 * to take them: its first one, a reconnect, or home moving to a machine that is
 * already live (connection.js followHomeContext).
 */
export async function flushCaptures() {
  if (!queue.length || !canSend()) return;
  const deviceId = captureDeviceId();
  const { sent, remaining } = await flushCaptureQueue(queue, (text) => homeCall("capture.create", { text }));
  queue = saveCaptureQueue(remaining);
  await Promise.all(sent.map(({ capture }) => track(capture, deviceId)));
  announce();
  if (sent.length) await refreshFeed();
}

/**
 * Keep the sent-and-watched captures current.
 *
 * While a capture is unfinished business the feed carries it, and the feed's
 * copy is the record. Once it drops off, the route is settled: ask once where
 * it went, and keep the row for a little while so the user can see — and undo —
 * the decision. A capture whose record has gone is dropped rather than shown.
 */
function syncTracked() {
  if (!tracked.size) return;
  const onFeed = new Map(
    feed.items.filter((row) => row.kind === "capture").map((row) => [row.capture_id, row]),
  );
  let changed = false;
  for (const [id, entry] of tracked) {
    const live = onFeed.get(id);
    if (live) {
      // The feed's copy is the record. Adopting it every tick would repaint the
      // inbox twice a tick, so it is adopted only when it actually moved.
      if (live !== entry.row || entry.settledAt) {
        entry.row = live;
        entry.inFeed = true;
        entry.settledAt = null;
        entry.settling = false;
        changed = true;
      }
      continue;
    }
    if (entry.settledAt || entry.settling || !canSend()) continue;
    entry.settling = true;
    entry.inFeed = false;
    changed = true;
    const startedRevision = entry.revision;
    homeCall("capture.get", { capture_id: id })
      .then(async (capture) => {
        if (tracked.get(id) !== entry || entry.inFeed) return;
        entry.settledAt = Date.now();
        entry.settling = false;
        if (entry.revision === startedRevision) {
          await writeCached(captureRecordAddress(entry.deviceId, id), capture);
        }
        await readTracked(id);
      })
      .catch(() => forgetCaptureRecord(id));
  }
  if (changed) announce();
}

// ---- the box ------------------------------------------------------------------

function paintPrompt() {
  const host = $("#compose");
  if (!host) return;
  host.innerHTML = composePromptHtml();
  host.querySelector("#compose-open").onclick = () => openCompose();
}

/** The manual panel: where the work goes, what it becomes there, and which
 *  harness picks it up. Rendered only while its disclosure is open. */
function advancedHtml() {
  const branches = branchOptions(feed.items, box.projectId);
  const projectOptions = feed.projects
    .map(
      (project) =>
        `<option value="${esc(project.id)}"${project.id === box.projectId ? " selected" : ""}>${esc(project.name || project.id)}</option>`,
    )
    .join("");
  return `<div class="compose-advanced">
    <label for="compose-project">Project</label>
    <select id="compose-project">${projectOptions || '<option value="">No projects on this device</option>'}</select>
    <div class="compose-kinds"><button class="btn mini primary" type="button" data-compose-kind="branch">Branch</button></div>
    <label for="compose-branch">Branch</label>
           <input id="compose-branch" type="text" class="path" list="compose-branches" ${fieldTraits("identifier")}
             placeholder="a new branch, named after what you said" value="${esc(box.branch)}" />
           <datalist id="compose-branches">${branches
             .map((branch) => `<option value="${esc(branch)}"></option>`)
             .join("")}</datalist>
    ${agentChoicePanelHtml(box.catalog, box.choice, { prefix: CHOICE_PREFIX, open: box.choiceOpen })}
    <button class="btn mini primary compose-manual" id="compose-manual-go" type="button"${box.busy ? " disabled" : ""}>${
      "Dispatch to the branch"
    }</button>
  </div>`;
}

function paintBox({ focus = true } = {}) {
  const host = $("#compose");
  if (!host || !box) return;
  const caret = focus ? null : selectionOf(host);
  const deviceName = creationDeviceName();
  host.innerHTML = composeBoxHtml({
    value: box.value,
    placeholder: composePlaceholder(deviceName),
    note: canSend() ? "" : composeOfflineNote(queue.length, deviceName),
    error: box.error,
    busy: box.busy,
    advanced: box.advancedOpen ? advancedHtml() : "",
  });
  if (box.gitOffer) host.querySelector(".compose-row").insertAdjacentHTML("afterend", composeGitOfferHtml(box.gitOffer));
  box.renderedAdvancedOpen = box.advancedOpen;
  box.renderedSnapshot = structuredClone(boxSnapshot());
  wireBox(host);
  const text = host.querySelector("#compose-text");
  if (focus) {
    text.focus();
    text.setSelectionRange(box.value.length, box.value.length);
  } else if (caret !== null) {
    text.setSelectionRange(caret, caret);
  }
}

const selectionOf = (host) => {
  const text = host.querySelector("#compose-text");
  return text && document.activeElement === text ? text.selectionStart : null;
};

function wireBox(host) {
  const text = host.querySelector("#compose-text");
  text.oninput = () => {
    box.value = text.value;
    if (box.renderedSnapshot) box.renderedSnapshot.value = text.value;
    saveBox(true);
  };
  text.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submitCapture();
    }
  };
  host.querySelector("#compose-cancel").onclick = () => closeCompose();
  host.querySelector("#compose-send").onclick = () => submitCapture();
  host.querySelector("#compose-advanced").onclick = () => {
    box.value = text.value;
    box.advancedOpen = !box.advancedOpen;
    saveBox();
  };
  const initGit = host.querySelector("[data-compose-init-git]");
  if (initGit) initGit.onclick = () => initializeGitThenDispatch();
  if (box.advancedOpen) wireAdvanced(host);
}

function wireAdvanced(host) {
  const project = host.querySelector("#compose-project");
  if (project) {
    project.onchange = () => {
      box.projectId = project.value;
      box.branch = "";
      box.gitOffer = null;
      saveBox();
    };
  }
  const branchKind = host.querySelector('[data-compose-kind="branch"]');
  if (branchKind) branchKind.onclick = () => {};
  const branch = host.querySelector("#compose-branch");
  if (branch) {
    branch.oninput = () => {
      box.branch = branch.value;
      if (box.renderedSnapshot) box.renderedSnapshot.branch = branch.value;
      saveBox(true);
    };
  }
  host.querySelector("#compose-manual-go").onclick = () => submitManual();
  wireChoice(host);
}

/** The three harness selects. A model belongs to its provider and an effort to
 *  its model, so a change repaints the panel — and only the panel, so the box
 *  above it keeps what was typed. */
function wireChoice(host) {
  const holder = host.querySelector(".agent-choice");
  if (!holder) return;
  holder.querySelector("[data-agent-choice-toggle]").onclick = () => {
    box.choiceOpen = !box.choiceOpen;
    saveBox();
  };
  // A model belongs to its provider, so a new provider starts from that
  // harness's own saved model and effort rather than the old one's.
  const onChange = (changed) => () => {
    const read = readAgentChoice(host, CHOICE_PREFIX);
    box.choice = changed.providerChanged ? agentDefaultsFor(read.provider) : reconcileAgentChoice(read, changed);
    saveBox();
  };
  const provider = holder.querySelector(`#${CHOICE_PREFIX}-provider`);
  if (provider) provider.onchange = onChange({ providerChanged: true });
  const model = holder.querySelector(`#${CHOICE_PREFIX}-model`);
  if (model) model.onchange = onChange({ modelChanged: true });
  const effort = holder.querySelector(`#${CHOICE_PREFIX}-effort`);
  if (effort) effort.onchange = onChange({});
}

function repaintChoice(host) {
  const holder = host.querySelector(".agent-choice");
  if (!holder) return;
  holder.outerHTML = agentChoicePanelHtml(box.catalog, box.choice, { prefix: CHOICE_PREFIX, open: box.choiceOpen });
  box.renderedSnapshot = structuredClone(boxSnapshot());
  wireChoice(host);
}

function repaintCatalogChoice() {
  const host = $("#compose");
  if (!host) return;
  const focused = host.ownerDocument.activeElement;
  const focusId = focused?.id?.startsWith(`${CHOICE_PREFIX}-`) ? focused.id : null;
  repaintChoice(host);
  if (focusId) host.querySelector(`#${focusId}`)?.focus();
}

/** The catalog is the creation device's, so the panel opens on what the box
 *  has — nothing, the first time it is opened — and repaints once that machine
 *  answers. A repaint mid-typing is avoided by only doing it when the answer
 *  actually changed, and a box closed before the answer lands takes its
 *  question with it. */
function loadCatalogForPanel() {
  const asked = box;
  const context = homeContext();
  let heardCatalog = false;
  const showCatalog = (loaded) => {
    if (box !== asked || !box.advancedOpen || !loaded || loaded === box.catalog) return;
    box.catalog = loaded;
    // Preserve a choice made while the catalog was refreshing. Only the
    // initially empty choice takes its default from the catalog.
    if (!box.choice.provider) box.choice = agentDefaultsIn(loaded);
    repaintCatalogChoice();
  };
  box.stopCatalog?.();
  box.stopCatalog = context?.onModelCatalogChanged((loaded) => {
    heardCatalog = true;
    showCatalog(loaded);
  }) || null;
  deviceCatalog(null)
    .then((loaded) => { if (!heardCatalog) showCatalog(loaded); })
    .catch(() => {});
}

// ---- opening, closing, sending ------------------------------------------------

const savedBoxFields = (saved, current) => ({
  value: saved.value,
  branch: saved.branch || "",
  projectId: saved.projectId || current.projectId,
  advancedOpen: Boolean(saved.advancedOpen),
  choiceOpen: Boolean(saved.choiceOpen),
  choice: saved.choice || current.choice,
});

function syncAdvancedCatalog(wasAdvanced) {
  if (wasAdvanced && !box.advancedOpen) {
    box.stopCatalog?.();
    box.stopCatalog = null;
  } else if (!wasAdvanced && box.advancedOpen) loadCatalogForPanel();
}

function restoreComposeBox(saved, opened) {
  if (box !== opened || !saved || typeof saved.value !== "string") return;
  const restored = savedBoxFields(saved, box);
  // The cache announces our own writes too. Replacing the box for an identical
  // readback drops the live control between a keystroke and the next click.
  if (JSON.stringify(restored) === JSON.stringify(box.renderedSnapshot)) return;
  const wasAdvanced = box.renderedAdvancedOpen;
  const focusedId = document.activeElement?.id;
  Object.assign(box, restored);
  paintBox({ focus: false });
  if (focusedId) $("#compose")?.querySelector(`#${focusedId}`)?.focus();
  syncAdvancedCatalog(wasAdvanced);
}

async function clearBoxDraft() {
  await box?.state?.write({ ...boxSnapshot(), value: "" });
}

export function openCompose() {
  if (!$("#compose")) return;
  if (box) {
    $("#compose-text")?.focus();
    return;
  }
  box = {
    value: "",
    error: "",
    busy: false,
    // What the creation device offers to start work with, as far as this box
    // knows: nothing until that machine has answered the panel's question.
    catalog: UNASKED_CATALOG,
    advancedOpen: false,
    renderedAdvancedOpen: false,
    renderedSnapshot: null,
    choiceOpen: false,
    choice: loadAgentDefaults(),
    kind: "branch",
    branch: "",
    projectId: (feed.projects[0] || {}).id || "",
    // The Initialize Git offer a plain folder's refusal became, or null.
    gitOffer: null,
  };
  const opened = box;
  box.state = watchUiState(
    uiAddress({ deviceId: captureDeviceId() || "", view: "compose", kind: "draft" }),
    (saved) => restoreComposeBox(saved, opened),
    { debounceMs: 180 },
  );
  paintBox();
}

export function closeCompose() {
  if (!box) return;
  box.state?.dispose();
  box.stopCatalog?.();
  box = null;
  paintPrompt();
}

function fail(message) {
  box.error = message;
  box.busy = false;
  paintBox({ focus: false });
}

async function submitCapture() {
  if (!box || box.busy) return;
  box.value = $("#compose-text")?.value ?? box.value;
  const text = box.value.trim();
  if (!text) {
    fail("Say what you want to get done first.");
    return;
  }
  if (!canSend()) {
    hold(text);
    await clearBoxDraft();
    closeCompose();
    return;
  }
  box.busy = true;
  box.error = "";
  paintBox({ focus: false });
  try {
    await track(await homeCall("capture.create", { text }), captureDeviceId());
    await clearBoxDraft();
    closeCompose();
    await refreshFeed();
  } catch {
    // The text is the one thing the user cannot produce again: a device that
    // would not take it holds it here instead of losing it.
    hold(text);
    await clearBoxDraft();
    closeCompose();
  }
}

async function submitManual() {
  if (!box || box.busy) return;
  box.value = $("#compose-text")?.value ?? box.value;
  const text = box.value.trim();
  if (!text) {
    fail("Say what the agent should do first.");
    return;
  }
  if (!box.projectId) {
    fail("No project to create in.");
    return;
  }
  if (!canSend()) {
    fail(composeManualAwayNote(creationDeviceName()));
    return;
  }
  box.busy = true;
  box.error = "";
  box.gitOffer = null;
  paintBox({ focus: false });
  const { method, params } = manualRoute({
    kind: box.kind,
    projectId: box.projectId,
    text,
    branch: box.branch,
    agentParams: agentChoiceParams(box.catalog, box.choice),
  });
  try {
    const created = await replyOrNothing(homeCall(method, params));
    settleManualRoute(manualRouteDestination(box.kind, created, box.projectId));
  } catch (error) {
    if (refusedForNoGit(error)) offerGitInitialization("");
    else fail(messageOf(error));
  }
}

/** A branch needs git and this project has none: the box offers to initialize
 *  it, keeping what was typed, instead of printing the refusal. */
function offerGitInitialization(error) {
  box.gitOffer = { error, pending: false };
  box.busy = false;
  paintBox({ focus: false });
}

/** Initialize Git where the user asked to, then dispatch what the box holds
 *  again. The box it was asked in is the one it answers, if it is still open. */
async function initializeGitThenDispatch() {
  const asked = box;
  if (!asked?.gitOffer || asked.gitOffer.pending) return;
  asked.gitOffer = { error: "", pending: true };
  paintBox({ focus: false });
  try {
    await homeCall("project.init_git", { project_id: asked.projectId });
  } catch (error) {
    if (box === asked) offerGitInitialization(messageOf(error));
    return;
  }
  if (box !== asked) return;
  asked.gitOffer = null;
  await submitManual();
}

/** The box is done with: shut it, re-read the board, and open what was made
 *  wherever the reply named it — on the machine it was dispatched to, since
 *  that is the only machine the new branch is on. */
function settleManualRoute(destination) {
  void clearBoxDraft();
  closeCompose();
  refreshFeed();
  if (destination) go({ ...destination, deviceId: captureDeviceId() });
}

// ---- mounting -----------------------------------------------------------------

/** Mount once, before the gate: compose is the one surface that works without a
 *  device. Re-entrant — a reconnect calls this again and it just repaints. */
export function initCompose() {
  if (mounted) {
    if (!box) paintPrompt();
    return;
  }
  mounted = true;
  queue = loadCaptureQueue();
  subscribeFeed((next) => {
    feed = deviceFeedView(next);
    syncTracked();
  });
  if (!$("#compose")) return;
  paintPrompt();
  document.addEventListener("keydown", (event) => {
    // A modal is a question in flight; opening a box behind it would answer
    // neither.
    if (isConfirmOpen()) return;
    if (composeShortcutFires(event)) {
      event.preventDefault();
      openCompose();
      return;
    }
    if (event.key === "Escape" && box) closeCompose();
  });
}
