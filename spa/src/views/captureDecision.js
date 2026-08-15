// The capture decision route's page: what should happen with one capture.
//
// Thin by design — the surface is core/captureDecisionView.js, which owns the
// paint, the four ways of deciding and the read. This file is the route's host:
// it names the capture and gives the surface its cadence.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { whenVisible } from "../core/visibility.js";
import { mountCaptureDecision } from "../core/captureDecisionView.js";
import "../styles/shell.css";

// The router runs while this page is open, so it is read at the feed's own
// pace: the states it moves through are the point of the page.
const POLL_MS = 2000;

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
  App.poll = setInterval(whenVisible(surface.load), POLL_MS);
  App.viewDispose = () => surface.dispose();
}
