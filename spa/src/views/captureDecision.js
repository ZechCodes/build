// The capture decision route's page: what should happen with one capture.
//
// Thin by design — the surface is core/captureDecisionView.js, which owns the
// paint, the four ways of deciding and the read. This file is the route's host:
// it names the capture and says when the surface reads again.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { subscribeCache } from "../core/localCache.js";
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
  // record moves. Captures were not in the cache-first brief and carry no
  // record of their own, so this is the whole of what wakes the page: a pass
  // writing that record. If it proves too little it is a follow-up here, not a
  // poll back.
  const unwatch = subscribeCache({}, (address) => {
    if (address.kind === "feed") void surface.load();
  });
  App.poll = { dispose: unwatch };
  App.viewDispose = () => {
    unwatch();
    surface.dispose();
  };
}
