// Where a pre-redesign URL waits. Those URLs name a run, a worktree, a plan or
// a primary checkout by id; the new ones name a branch by (project, branch) or
// an issue by (project, issue). The feed carries both halves, so this surface
// holds the screen for one snapshot, rewrites the hash to the work item the id
// belongs to, and gets out of the way. Nothing carries it → the inbox.
//
// Those URLs name one machine's work — the home device's, until a route can
// name its own — so the rows they are looked up in are that machine's: every
// machine mints a `proj-1`, and a primary checkout is named by its project
// alone. Which is also why it waits for that machine's own answer and not
// merely for the first one to arrive.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { resolveLegacyRoute } from "../core/routeResolve.js";
import { homeContext, deviceFeedView } from "../core/deviceContexts.js";
import "../styles/shell.css";

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
    go(resolveLegacyRoute(reference, deviceFeedView(feed).items) || { name: "inbox" });
  });
  App.viewDispose = unsubscribe;
}

/**
 * Whether this snapshot is the one to look the link up in.
 *
 * Every device seeds from its cache and answers on its own schedule, so the
 * first snapshot delivered is usually somebody else's rows and says nothing
 * about a link that names the home device's work. Wait for that device's own
 * answer — unless it is in no position to give one, where the boot paint it
 * left behind is everything there is and a link has to land somewhere.
 */
function answersThisLink(feed) {
  const home = homeContext();
  if (!home?.session || home.offline) return true;
  const view = feed.devices?.[home.deviceId];
  return Boolean(view) && !view.cached;
}
