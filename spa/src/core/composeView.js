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
import { canAnswer, deviceFeedView, homeContext } from "./deviceContexts.js";
import { deviceCall, deviceCatalog } from "./inboxDevices.js";
import { creationDeviceId, deviceNameOf } from "./devicePolicy.js";
import { UNASKED_CATALOG } from "./modelCatalog.js";
import { loadAgentDefaults } from "./agentDefaults.js";
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
import "../styles/shell.css";

const CHOICE_PREFIX = "compose-choice";

let queue = []; // captures this client is holding for an absent device
const tracked = new Map(); // capture id → { row, settledAt, settling }
const listeners = new Set(); // who repaints when the held captures change
// The home device's slice of the feed, never the merge: a capture goes to the
// device creation goes to, so the destinations this box offers and the project
// names it prints are that device's. Every device mints a `proj-1`.
let feed = { items: [], projects: [] };
let box = null; // the open box's state, or null while it is shut
let mounted = false;
// Every call this box makes is the creation device's, asked for the way every
// other surface asks: by the device it is about, with naming none meaning home.
// A machine that cannot answer hands back a caller that refuses, so nothing
// here asks whether a device is there — only canSend, which is the same
// question the note and the queue are the answer to.
const homeCall = () => deviceCall(null);
const canSend = () => canAnswer(homeContext());
/** What the account calls the machine this box sends to, while it can name it:
 *  the creation device, online or away. */
const creationDeviceName = () => deviceNameOf(App.devices, creationDeviceId(App.devices, App.selectedDeviceId));
const projectNameOf = (projectId) =>
  (feed.projects.find((project) => project.id === projectId) || {}).name || projectId || "";

// ---- what the client is holding ----------------------------------------------

/** The capture rows this client owns: the ones it is holding for an absent
 *  device, and the ones it has sent and is still watching. The inbox merges
 *  these with the feed's own, which win on a tie. */
export function pendingCaptureRows(nowMs = Date.now()) {
  for (const [id, entry] of tracked) {
    if (routedCaptureExpired({ state: entry.row.state, settledAt: entry.settledAt }, nowMs)) tracked.delete(id);
  }
  return [...queue.map(queuedCaptureRow), ...[...tracked.values()].map((entry) => entry.row)];
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
export function adoptCaptureRecord(capture) {
  const held = capture && capture.id ? tracked.get(capture.id) : null;
  if (!held) return;
  const projectName = capture.routing ? projectNameOf(capture.routing.project_id) : "";
  tracked.set(capture.id, {
    row: captureRow(capture, { projectName }),
    settledAt: held.settledAt ? Date.now() : null,
    settling: false,
  });
  announce();
}

export function forgetCaptureRecord(captureId) {
  if (!tracked.delete(captureId)) return;
  announce();
}

/** Repaint when the held captures change. Returns unsubscribe. */
export function subscribePendingCaptures(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function hold(text) {
  queue = saveCaptureQueue(
    [...queue, queuedCapture(text, { id: `local-${Date.now()}-${queue.length}`, createdAt: new Date().toISOString() })],
  );
  announce();
}

function track(capture) {
  const projectName = capture.routing ? projectNameOf(capture.routing.project_id) : "";
  tracked.set(capture.id, { row: captureRow(capture, { projectName }), settledAt: null, settling: false });
  announce();
}

/**
 * Send what the client is holding, oldest first. Called on every fresh session
 * (connection.js adopts one) — the gate, a reconnect, a device switch.
 */
export async function flushCaptures() {
  if (!queue.length || !canSend()) return;
  const { sent, remaining } = await flushCaptureQueue(queue, (text) => homeCall()("capture.create", { text }));
  queue = saveCaptureQueue(remaining);
  sent.forEach(({ capture }) => track(capture));
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
        tracked.set(id, { row: live, settledAt: null, settling: false });
        changed = true;
      }
      continue;
    }
    if (entry.settledAt || entry.settling || !canSend()) continue;
    entry.settling = true;
    changed = true;
    homeCall()("capture.get", { capture_id: id })
      .then((capture) => {
        const projectName = capture.routing ? projectNameOf(capture.routing.project_id) : "";
        tracked.set(id, { row: captureRow(capture, { projectName }), settledAt: Date.now(), settling: false });
      })
      .catch(() => tracked.delete(id))
      .then(announce);
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
  const kindButton = (kind, label) =>
    `<button class="btn mini${box.kind === kind ? " primary" : ""}" type="button" data-compose-kind="${kind}">${label}</button>`;
  return `<div class="compose-advanced">
    <label for="compose-project">Project</label>
    <select id="compose-project">${projectOptions || '<option value="">No projects on this device</option>'}</select>
    <div class="compose-kinds">${kindButton("issue", "Issue")}${kindButton("branch", "Branch")}</div>
    ${
      box.kind === "branch"
        ? `<label for="compose-branch">Branch</label>
           <input id="compose-branch" type="text" class="path" list="compose-branches" autocomplete="off"
             placeholder="a new branch, named after what you said" value="${esc(box.branch)}" />
           <datalist id="compose-branches">${branches
             .map((branch) => `<option value="${esc(branch)}"></option>`)
             .join("")}</datalist>`
        : `<div class="compose-note dim">Nothing runs until you open it and send the first message.</div>`
    }
    ${agentChoicePanelHtml(box.catalog, box.choice, { prefix: CHOICE_PREFIX, open: box.choiceOpen })}
    <button class="btn mini primary compose-manual" id="compose-manual-go" type="button"${box.busy ? " disabled" : ""}>${
      box.kind === "branch" ? "Dispatch to the branch" : "File the issue"
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
    paintBox({ focus: false });
    if (box.advancedOpen) loadCatalogForPanel();
  };
  if (box.advancedOpen) wireAdvanced(host);
}

function wireAdvanced(host) {
  const project = host.querySelector("#compose-project");
  if (project) {
    project.onchange = () => {
      box.projectId = project.value;
      box.branch = "";
      paintBox({ focus: false });
    };
  }
  host.querySelectorAll("[data-compose-kind]").forEach((control) => {
    control.onclick = () => {
      box.value = host.querySelector("#compose-text").value;
      box.kind = control.dataset.composeKind;
      paintBox({ focus: false });
    };
  });
  const branch = host.querySelector("#compose-branch");
  if (branch) {
    branch.oninput = () => {
      box.branch = branch.value;
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
    repaintChoice(host);
  };
  const onChange = (changed) => () => {
    box.choice = reconcileAgentChoice(readAgentChoice(host, CHOICE_PREFIX), changed);
    repaintChoice(host);
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
  wireChoice(host);
}

/** The catalog is the creation device's, so the panel opens on what the box
 *  has — nothing, the first time it is opened — and repaints once that machine
 *  answers. A repaint mid-typing is avoided by only doing it when the answer
 *  actually changed, and a box closed before the answer lands takes its
 *  question with it. */
function loadCatalogForPanel() {
  const asked = box;
  deviceCatalog(null)
    .then((loaded) => {
      if (box !== asked || loaded === box.catalog) return;
      box.catalog = loaded;
      if (box.advancedOpen) paintBox({ focus: false });
    })
    .catch(() => {});
}

// ---- opening, closing, sending ------------------------------------------------

export function openCompose() {
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
    choiceOpen: false,
    choice: loadAgentDefaults(),
    kind: "issue",
    branch: "",
    projectId: (feed.projects[0] || {}).id || "",
  };
  paintBox();
}

export function closeCompose() {
  if (!box) return;
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
    closeCompose();
    return;
  }
  box.busy = true;
  box.error = "";
  paintBox({ focus: false });
  try {
    track(await homeCall()("capture.create", { text }));
    closeCompose();
    await refreshFeed();
  } catch {
    // The text is the one thing the user cannot produce again: a device that
    // would not take it holds it here instead of losing it.
    hold(text);
    closeCompose();
  }
}

async function submitManual() {
  if (!box || box.busy) return;
  box.value = $("#compose-text")?.value ?? box.value;
  const text = box.value.trim();
  if (!text) {
    fail(box.kind === "branch" ? "Say what the agent should do first." : "Describe the issue first.");
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
  paintBox({ focus: false });
  const { method, params } = manualRoute({
    kind: box.kind,
    projectId: box.projectId,
    text,
    branch: box.branch,
    agentParams: agentChoiceParams(box.catalog, box.choice),
  });
  try {
    const created = await replyOrNothing(homeCall()(method, params));
    settleManualRoute(manualRouteDestination(box.kind, created, box.projectId));
  } catch (error) {
    fail(messageOf(error));
  }
}

/** The box is done with: shut it, re-read the board, and open what was made
 *  wherever the reply named it. */
function settleManualRoute(destination) {
  closeCompose();
  refreshFeed();
  if (destination) go(destination);
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
  paintPrompt();
  subscribeFeed((next) => {
    feed = deviceFeedView(next);
    syncTracked();
  });
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
