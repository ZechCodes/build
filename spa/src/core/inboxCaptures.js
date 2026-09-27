// What a capture row in the rail can do to its route: retry one that gave up,
// and send the capture somewhere else. Both go through the daemon's own capture
// verbs — a reroute by hand and a route by the router are the same kind of
// thing afterwards — and both go to the device that is holding the capture.
//
// Answering the router is not one of them. What to do with a capture is a
// decision with several shapes — the router's own choices, a destination named
// by hand, words, or abandoning it — and the row opens the page that holds all
// of them (views/captureDecision.js) rather than hosting the thinnest one.
//
// The rail lends this module its repaint and its record of what it painted; the
// state of the destination picker, and what a refused reroute left on a row,
// live here.

import { $ } from "../dom.js";
import { adoptCaptureRecord } from "./composeView.js";
import { refreshFeed } from "./taskFeed.js";
import { verbCall } from "./inboxDevices.js";
import { messageOf } from "./text.js";
import { uiAddress, watchUiState } from "./localUiState.js";

let repaint = () => {};
let entryFor = () => null;

/** What the rail lends this module: the repaint that shows what a press did,
 *  and the entry behind a row key — a row is never found by a selector built
 *  out of an id the daemon minted. */
export function initCaptureRows({ onChange, entryOf }) {
  pickerRecord?.dispose({ flushPending: false });
  repaint = onChange;
  entryFor = entryOf;
  pickerRecord = watchUiState(uiAddress({ view: "inbox-captures", kind: "menu" }), (saved) => {
    rerouteKey = saved?.key || null;
    rerouteBranchProject = saved?.branchProject || null;
    repaint();
  });
}

export function disposeCaptureRows() {
  pickerRecord?.dispose({ flushPending: false });
  pickerRecord = null;
  repaint = () => {};
  entryFor = () => null;
}

let rerouteKey = null; // the capture row whose destination picker is open
let rerouteBranchProject = null; // the project in that picker whose branch field is open
let pickerRecord = null;
const capturesBeingRerouted = new Set();
const errors = new Map(); // capture id → the message its row is showing

/** The destination picker as the row painter needs it. */
export const reroutePicker = () => ({ rerouteKey, rerouteBranchProject });

/** What a refused reroute left on this row, if anything. */
export const captureError = (captureId) => errors.get(captureId);

/** One control per attribute a capture row paints, answered off the rail's one
 *  listener in the order a press is read in. */
export const CAPTURE_CONTROLS = [
  ["data-capture-retry", (control) => rerouteCapture(control.dataset.captureRetry, null)],
  ["data-capture-reroute", (control) => openPicker(control.dataset.captureReroute)],
  // Branch is the one destination with something left to say, so it discloses
  // the field that says it instead of dispatching on the spot.
  ["data-reroute-branch-open", (control) => openRerouteBranch(control.dataset.rerouteBranchOpen)],
  ["data-reroute-project", (control) => dispatchReroute(control)],
];

function openPicker(captureId) {
  const key = `capture:${captureId}`;
  void pickerRecord?.write({ key: rerouteKey === key ? null : key, branchProject: null });
}

function openRerouteBranch(projectId) {
  const branchProject = rerouteBranchProject === projectId ? null : projectId;
  void pickerRecord?.write({ key: rerouteKey, branchProject }).then(() => {
  // The field is found through the list that was just painted, never through a
  // selector built out of an id the daemon minted.
    if (branchProject) $("#inbox-list")?.querySelector("[data-reroute-branch]")?.focus();
  });
}

function dispatchReroute(control) {
  const row = control.closest(".capture-entry");
  const named = control.dataset.rerouteKind === "branch" ? branchFieldValue(control) : "";
  void pickerRecord?.write({ key: null, branchProject: null });
  rerouteCapture(row.dataset.capture, {
    projectId: control.dataset.rerouteProject,
    kind: control.dataset.rerouteKind,
    branch: named,
  });
}

/** Enter in the branch field is the Dispatch beside it. */
export function onCaptureKeydown(event) {
  if (event.key !== "Enter") return;
  const field = event.target.closest("[data-reroute-branch]");
  if (!field) return;
  event.preventDefault();
  field.closest(".reroute-branch").querySelector("[data-reroute-kind='branch']").click();
}

/** The branch named beside a Dispatch button, "" when the field is empty or
 *  the destination was chosen without one. */
function branchFieldValue(control) {
  const field = control.closest(".reroute-branch")?.querySelector("[data-reroute-branch]");
  return field ? field.value.trim() : "";
}

/** With a destination this routes by hand; with none it re-fires the router,
 *  which is what the retry on a failed route is. */
async function rerouteCapture(captureId, destination) {
  if (capturesBeingRerouted.has(captureId)) return;
  capturesBeingRerouted.add(captureId);
  errors.delete(captureId);
  try {
    const entry = entryFor(`capture:${captureId}`);
    const call = verbCall(entry);
    const rerouted = await call("capture.reroute", rerouteParams(captureId, destination));
    // The answer carries the new routing, and for a capture that has already
    // settled it is the only thing that will: the feed stopped carrying it, so
    // nothing else would ever correct the row's "→ project as task".
    await adoptCaptureRecord(rerouted, entry?.deviceId);
    await refreshFeed();
  } catch (error) {
    errors.set(captureId, messageOf(error));
  } finally {
    capturesBeingRerouted.delete(captureId);
    repaint();
  }
}

/** What a reroute asks for: a destination, or nothing at all — which is the
 *  retry, and means "decide again". A branch carries the name when one was
 *  given; with none the daemon names it after what was said. */
function rerouteParams(captureId, destination) {
  if (!destination) return { capture_id: captureId };
  const params = { capture_id: captureId, project_id: destination.projectId, kind: destination.kind };
  return destination.branch ? { ...params, branch: destination.branch } : params;
}
