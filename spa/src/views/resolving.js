// Where a URL that does not say enough waits. A pre-redesign URL names a run, a
// worktree or a plan by id; the new ones name a branch by
// (project, branch) and a task by (project, task), and a work URL that names
// no device names a project every machine mints its own `proj-1` of. The feed
// carries what is missing, so this surface holds the screen until the devices
// that can answer have, rewrites the hash to the work item, and gets out of the
// way. Nothing carries it → the inbox.
//
// Which machine's copy a link opens when several carry it is the same policy
// creation follows: the home device, else the first device the account lists
// that is online (core/routeResolve.js).

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { resolveLegacyRoute } from "../core/routeResolve.js";
import { homeDeviceId } from "../core/devicePolicy.js";
import { liveContexts } from "../core/deviceContexts.js";
import "../styles/shell.css";

/** How long a link waits for the machines that have not answered yet. A device
 *  whose session is up but whose board.list keeps failing writes no view at all,
 *  and a link that waited on it would spin for as long as that bridge stays
 *  sick; past this, the rows that did arrive are the best answer there is. */
const WAIT_FOR_DEVICES_MS = 3000;

/** Which device wins a link several of them could open. */
const devicePolicy = () => ({
  homeDeviceId: homeDeviceId(App.devices, App.selectedDeviceId),
  deviceOrder: App.devices.filter((device) => device.status === "online").map((device) => device.id),
});

export function renderResolving() {
  const root = $("#root");
  root.className = "surface";
  root.innerHTML = `
    <div class="shell-stub">
      <h2>Opening…</h2>
      <p>That link points at work Build now shows as a branch or a task. Finding it.</p>
    </div>`;
  const reference = App.route;
  let settled = false;
  let latest = null; // the merge as it last stood, whether or not everyone has spoken
  /** Open what this link turns out to mean, once — on the inbox when the feed
   *  in hand carries nothing by that id. */
  const land = () => {
    if (settled) return;
    settled = true;
    go(resolveLegacyRoute(reference, latest, devicePolicy()) || { name: "inbox" });
  };
  const unsubscribe = subscribeFeed((feed) => {
    latest = feed;
    if (answersThisLink(feed)) land();
  });
  // A snapshot already in hand answers above, before this line: the link has
  // landed and the surface that took its place owns the teardown now.
  if (settled) {
    unsubscribe();
    return;
  }
  const patience = setTimeout(land, WAIT_FOR_DEVICES_MS);
  App.viewDispose = () => {
    clearTimeout(patience);
    unsubscribe();
  };
}

/**
 * Whether this snapshot is the one to look the link up in.
 *
 * Every device seeds from its cache and answers on its own schedule, so the
 * first snapshot delivered is usually one machine's rows and says nothing about
 * whether another one holds a better answer to the same id. Wait until every
 * device that can answer has — unless none is in a position to, where the boot
 * paint they left behind is everything there is and a link has to land
 * somewhere. The wait itself is bounded by the caller: a machine can be up and
 * still never answer.
 */
function answersThisLink(feed) {
  const live = liveContexts();
  if (!live.length) return true;
  return live.every((context) => {
    const view = feed.devices?.[context.deviceId];
    return Boolean(view) && !view.cached;
  });
}
