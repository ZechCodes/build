// Where a URL that does not say enough waits. A pre-redesign URL names a run, a
// worktree, a plan or a primary checkout by id; the new ones name a branch by
// (project, branch) and an issue by (project, issue), and a work URL that names
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
      <p>That link points at work Build now shows as a branch or an issue. Finding it.</p>
    </div>`;
  const reference = App.route;
  let settled = false;
  const unsubscribe = subscribeFeed((feed) => {
    if (settled || !answersThisLink(feed)) return;
    settled = true;
    go(resolveLegacyRoute(reference, feed, devicePolicy()) || { name: "inbox" });
  });
  App.viewDispose = unsubscribe;
}

/**
 * Whether this snapshot is the one to look the link up in.
 *
 * Every device seeds from its cache and answers on its own schedule, so the
 * first snapshot delivered is usually one machine's rows and says nothing about
 * whether another one holds a better answer to the same id. Wait until every
 * device that can answer has — unless none is in a position to, where the boot
 * paint they left behind is everything there is and a link has to land
 * somewhere.
 */
function answersThisLink(feed) {
  const live = liveContexts();
  if (!live.length) return true;
  return live.every((context) => {
    const view = feed.devices?.[context.deviceId];
    return Boolean(view) && !view.cached;
  });
}
