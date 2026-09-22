// The capture decision page's wiring: one capture, every way of deciding what
// happens to it, and the cache record that keeps its answer current.
//
// WHAT the page says is core/captureDecision.js; this module is the behaviour.
// Four ways out, and the page offers all of them at once: tap one of the
// choices the router offered, name a destination yourself, answer in words, or
// cancel the capture altogether.
//
// The paint is idempotent — a cache read of the same capture paints
// nothing — and it stands down entirely while a field on the page has the
// caret, because rewriting the page would take the words, the caret and, on a
// phone, the keyboard with it.

import { App, go } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { creationDevice, deviceFeedView } from "./deviceContexts.js";
import { confirmAction, isConfirmOpen } from "./confirm.js";
import { notifyError } from "./notify.js";
import { branchOptions } from "./compose.js";
import { esc, messageOf } from "./text.js";
import { deviceCall } from "./inboxDevices.js";
import { entryKeyOf } from "./inbox.js";
import { INBOX_SCOPE } from "./inboxView.js";
import { forgetCaptureRecord } from "./composeView.js";
import { removeRecord, runOptimistic } from "./optimistic.js";
import { captureRecordAddress } from "./captureRecords.js";
import { DEVICES_ADDRESS, deleteCached, readCached, readCachedMany, subscribeCache, writeCached } from "./localCache.js";
import {
  answerParams,
  captureCancelConfirm,
  captureDecisionHtml,
  captureDecisionModel,
  manualRouteAnswer,
  manualRouteParams,
} from "./captureDecision.js";
import "../styles/shell.css";

const EDITABLE = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/**
 * The machine a capture is on.
 *
 * A capture stays on the machine it was taken on and is routed into that
 * machine's own projects (00-multi-device-design.md §8) — and the inbox carries
 * every device's captures, so the row for this one says which machine that is.
 * Naming nobody means home, which is where a capture with no row yet was taken.
 */
const deviceOfCapture = (feed, captureId) =>
  (feed?.items || []).find((row) => row.kind === "capture" && row.capture_id === captureId)?.deviceId || null;

/**
 * Mount the decision page for one capture into `host`.
 *
 * Answers `{ load, dispose }`: `load` reads the capture once (the caller owns
 * the cadence), `dispose` stops it caring about anything that lands later.
 */
export function mountCaptureDecision(host, captureId) {
  let record = null; // the capture last read from the local cache
  let paintedFrom = null; // what the host currently stands on
  let disposed = false;
  let busy = false; // a mutation is in flight; the page holds still
  let error = "";
  let feed = { items: [], projects: [] };
  let captureDeviceId = null; // the machine this capture is on, once a snapshot says
  let cacheAddress = null;
  let cacheRevision = 0;
  let cacheRead = Promise.resolve();
  let unwatchCache = () => {};
  let unsubscribe = () => {};
  let lastFeed = null;
  // What the user has typed or chosen, kept beside the page rather than in it:
  // a repaint rebuilds the page, and these are theirs.
  const draft = { projectId: "", kind: "branch", branch: "", answer: "" };

  host.innerHTML = '<div class="empty">Reading the capture…</div>';

  // A capture is device-scoped: it is on the machine it was taken on, and the
  // projects it can be routed to are that machine's. A snapshot that has not
  // caught up with the capture leaves the last answer standing rather than
  // sending the page home mid-decision.
  /** Everything this page asks — read, answer, reroute and cancel alike — goes
   *  to the machine the capture is on, and is refused in the words its inbox row
   *  is greyed with while that machine is away. */
  const ask = (method, params) => deviceCall(captureDeviceId)(method, params);

  // ---- what the page stands on ------------------------------------------------

  const model = () => captureDecisionModel(record, { projects: feed.projects });

  /** The project the manual panel opens on: the one the router's own offer
   *  named, else the one it was routed to, else the first this device has. A
   *  project the feed has never heard of is no default at all — the selector
   *  could not show it. */
  // eslint-disable-next-line complexity -- ratchet: ensureProject is at 12, cap 10 — reduce it, then drop this line
  function ensureProject() {
    if (draft.projectId) return;
    const known = (projectId) => projectId && feed.projects.some((project) => project.id === projectId);
    const offered = (record?.question?.options || []).map((option) => option.project_id).find(known);
    draft.projectId = offered || (known(record?.routing?.project_id) ? record.routing.project_id : "") || (feed.projects[0] || {}).id || "";
  }

  /** The live fields, back into the draft. The DOM is the truth about what has
   *  been typed, and a repaint is about to throw it away. */
  function readFields() {
    const answerBox = host.querySelector("#capture-answer");
    if (answerBox) draft.answer = answerBox.value;
    const branchField = host.querySelector("#capture-branch");
    if (branchField) draft.branch = branchField.value;
  }

  /** Whether the user is in the middle of something the page must not disturb. */
  function interacting() {
    if (isConfirmOpen()) return true;
    const active = document.activeElement;
    return Boolean(active && host.contains(active) && EDITABLE.has(active.tagName));
  }

  /** The field with the caret, so a repaint the user DID ask for gives it back.
   *  The ids are this module's own literals, never anything the daemon minted. */
  function focusedField() {
    const active = document.activeElement;
    if (!active || !host.contains(active) || !active.id) return null;
    const caret = typeof active.selectionStart === "number" ? [active.selectionStart, active.selectionEnd] : null;
    return { id: active.id, caret };
  }

  function restoreFocus(previous) {
    if (!previous) return;
    const field = document.getElementById(previous.id);
    if (!field || !host.contains(field)) return;
    field.focus();
    if (previous.caret) field.setSelectionRange(previous.caret[0], previous.caret[1]);
  }

  function draw() {
    if (disposed) return;
    readFields();
    ensureProject();
    const page = model();
    const ui = {
      projects: feed.projects,
      branches: branchOptions(feed.items, draft.projectId),
      projectId: draft.projectId,
      kind: draft.kind,
      branch: draft.branch,
      answer: draft.answer,
      busy,
      error,
    };
    // What has been typed is deliberately not part of this: it is restored from
    // the draft whatever happens, and a keystroke is never a reason to rebuild
    // the page around it.
    const key = JSON.stringify([page, ui.projects, ui.branches, ui.projectId, ui.kind, busy, error]);
    if (key === paintedFrom) return;
    paintedFrom = key;
    const caret = focusedField();
    host.innerHTML = captureDecisionHtml(page, ui);
    wire();
    restoreFocus(caret);
  }

  /** The repaint the poll asks for, which is the one that must wait. */
  function drawFromPoll() {
    if (interacting()) return;
    draw();
  }

  /** Every paint of the daemon's full capture starts with this read. The
   *  announcement carries only an address, never the reply that caused it. */
  function rereadCapture() {
    const address = cacheAddress;
    const read = async () => {
      const held = await readCached(address);
      if (disposed || address !== cacheAddress) return;
      record = held?.value || null;
      if (record?.state === "routed") leaveForTheInbox();
      else if (record) drawFromPoll();
    };
    cacheRead = cacheRead.then(read, read);
    return cacheRead;
  }

  function watchCapture() {
    const address = captureRecordAddress(captureDeviceId, captureId);
    if (cacheAddress?.deviceId === address.deviceId) return cacheRead;
    unwatchCache();
    cacheAddress = address;
    cacheRevision = 0;
    record = null;
    paintedFrom = null;
    unwatchCache = subscribeCache(address, () => {
      cacheRevision += 1;
      void rereadCapture();
    });
    return rereadCapture();
  }

  async function writeCapture(capture) {
    if (capture?.id !== captureId || disposed) return;
    await writeCached(cacheAddress, capture);
    await rereadCapture();
  }

  /** A capture the daemon will not state. Whatever is already on screen stays;
   *  a first read that fails says so, and offers the way back. */
  function unreadable(message) {
    const key = `unreadable:${message}`;
    if (key === paintedFrom) return;
    paintedFrom = key;
    host.innerHTML = `<div class="empty gone">This capture could not be read — it may already have been routed or cancelled.
      <div class="dim">${esc(message)}</div>
      <button class="btn" id="capture-back" type="button">Back to the inbox</button></div>`;
    host.querySelector("#capture-back").onclick = () => go({ name: "inbox" });
  }

  // ---- the four ways out --------------------------------------------------------

  function wire() {
    host.querySelectorAll("[data-capture-option]").forEach((control) => {
      control.onclick = () => answer(answerParams(captureId, { optionId: control.dataset.captureOption }));
    });
    const project = host.querySelector("#capture-project");
    if (project) {
      project.onchange = () => {
        readFields();
        draft.projectId = project.value;
        // The branch belonged to the project that is no longer chosen.
        draft.branch = "";
        draw();
      };
    }
    const branchKind = host.querySelector('[data-capture-kind="branch"]');
    if (branchKind) branchKind.onclick = () => draw();
    const branchField = host.querySelector("#capture-branch");
    if (branchField) branchField.oninput = () => (draft.branch = branchField.value);
    const route = host.querySelector("#capture-route");
    if (route) route.onclick = () => routeByHand();
    const answerBox = host.querySelector("#capture-answer");
    if (answerBox) {
      answerBox.oninput = () => (draft.answer = answerBox.value);
      answerBox.onkeydown = (event) => {
        if (event.key !== "Enter" || !(event.metaKey || event.ctrlKey)) return;
        event.preventDefault();
        sendWords();
      };
    }
    const send = host.querySelector("#capture-answer-send");
    if (send) send.onclick = () => sendWords();
    const cancelControl = host.querySelector("#capture-cancel");
    if (cancelControl) cancelControl.onclick = () => cancel();
  }

  /** Where the capture now stands — or the way out of the page, once it stands
   *  somewhere. A routed capture is a decision made, and this page is only for
   *  the ones still undecided. */
  function settle() {
    if (disposed) return;
    if (record && record.state === "routed") {
      leaveForTheInbox();
      return;
    }
    draw();
  }

  function leaveForTheInbox() {
    refreshFeed();
    go({ name: "inbox" });
  }

  /** `capture.answer`, whichever way the answer was given. */
  async function answer(params) {
    if (!params || busy || disposed) return;
    busy = true;
    error = "";
    draw();
    try {
      const answered = await ask("capture.answer", params);
      draft.answer = ""; // said and gone
      await writeCapture(answered);
      await refreshFeed();
    } catch (failure) {
      error = messageOf(failure);
      notifyError("The router could not take that answer", error);
    } finally {
      busy = false;
      settle();
    }
  }

  function sendWords() {
    readFields();
    answer(answerParams(captureId, { text: draft.answer }));
  }

  /**
   * The destination the user named themselves.
   *
   * With a question on the table this IS the answer, in the same words the
   * bridge writes when one of the router's own options is tapped — so the
   * router hears a hand-picked destination in the terms it routes in, rather
   * than being talked over mid-question. With nothing asked there is nothing to
   * answer, and the destination is stated to the daemon instead.
   */
  async function routeByHand() {
    if (busy || disposed) return;
    readFields();
    if (!draft.projectId) return;
    if (model().awaitingAnswer) {
      answer(answerParams(captureId, { text: manualRouteAnswer(draft) }));
      return;
    }
    busy = true;
    error = "";
    draw();
    try {
      const routed = await ask("capture.reroute", manualRouteParams(captureId, draft));
      await writeCapture(routed);
      await refreshFeed();
    } catch (failure) {
      error = messageOf(failure);
      notifyError("The capture could not be routed", error);
    } finally {
      busy = false;
      settle();
    }
  }

  /** The way out of the decision itself. Destructive — the record goes — so it
   *  is outlined before it happens. */
  async function cancel() {
    if (busy || disposed) return;
    if (!(await confirmAction(captureCancelConfirm(model())))) return;
    go({ name: "inbox" });
    await runOptimistic({
      scope: INBOX_SCOPE,
      records: [removeRecord(entryKeyOf({ kind: "capture", capture_id: captureId }))],
      call: async () => {
        await ask("capture.cancel", { capture_id: captureId });
        forgetCaptureRecord(captureId);
        await deleteCached([cacheAddress]);
      },
      failureSummary: "The capture could not be cancelled",
    });
    await refreshFeed();
  }

  // ---- the read -----------------------------------------------------------------

  async function load() {
    if (disposed) return;
    await ready;
    await cacheRead;
    const address = cacheAddress;
    const startedRevision = cacheRevision;
    let capture;
    try {
      capture = await ask("capture.get", { capture_id: captureId });
    } catch (failure) {
      // A device that went away, or a capture that is no longer there. What is
      // already on screen stays; a first read that fails says so.
      if (!disposed && !record) unreadable(messageOf(failure));
      return;
    }
    if (disposed || address !== cacheAddress) return;
    if (cacheRevision !== startedRevision) {
      await rereadCapture();
      return;
    }
    await writeCapture(capture);
  }

  /** A direct capture link carries an id, not a device. The feed may already
   *  name its machine; on a cold mount, find its stored full record across the
   *  account's known devices before choosing where to read. */
  async function resolveCaptureDevice() {
    if (!captureDeviceId) {
      const storedDevices = (await readCached(DEVICES_ADDRESS))?.value || [];
      const deviceIds = [...new Set([...App.devices, ...storedDevices].map((device) => device?.id).filter(Boolean))];
      const records = await readCachedMany(deviceIds.map((deviceId) => captureRecordAddress(deviceId, captureId)));
      captureDeviceId = deviceIds.find((_, index) => records[index]?.value?.id === captureId) || creationDevice();
    }
    feed = deviceFeedView(lastFeed, captureDeviceId);
    await watchCapture();
  }

  unsubscribe = subscribeFeed((next) => {
    lastFeed = next;
    const onDevice = deviceOfCapture(next, captureId);
    if (onDevice && onDevice !== captureDeviceId) {
      captureDeviceId = onDevice;
      void watchCapture().then(() => {
        if (ready) void load();
      });
    }
    feed = deviceFeedView(next, captureDeviceId || creationDevice());
    if (record) drawFromPoll();
  });
  const ready = resolveCaptureDevice();

  return {
    load,
    dispose() {
      disposed = true;
      unsubscribe();
      unwatchCache();
    },
  };
}
