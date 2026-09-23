// The capture decision route's page: what should happen with one capture.
//
// Thin by design — the surface is core/captureDecisionView.js, which owns the
// paint, the four ways of deciding and the read. This file is the route's host:
// it names the capture and says when the surface reads again.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { subscribeBoardWrites } from "../core/feedRows.js";
import { mountCaptureDecision } from "../core/captureDecisionView.js";
import "../styles/shell.css";

export function renderCaptureDecision() {
  const root = $("#root");
  const captureId = App.route.id;
  if (!captureId) {
    go({ name: "inbox" });
    return;
  }
  root.innerHTML = '<div id="capture-page"></div>';
  const surface = mountCaptureDecision($("#capture-page"), captureId);
  // Not awaited: the page must be on screen even when the device is unreachable
  // and the read never lands.
  surface.load();
  // A capture IS a feed row, so the page reads again when a machine's board
  // moves — the whole list a pass writes, and the single row a `state` push
  // writes, which is what a capture moving from routing to awaiting-answer
  // actually is. The resulting capture.get reply writes the capture's own
  // cache record; that record's announcement repaints the page.
  // The read is handed over rather than fired and forgotten: what keeps a
  // pass's dozen row writes from being a dozen reads is the wake knowing when
  // this one is still out (core/feedRows.js).
  const unwatch = subscribeBoardWrites(() => surface.load());
  App.poll = { dispose: unwatch };
  App.viewDispose = () => {
    unwatch();
    surface.dispose();
  };
}
