// Where a pre-redesign URL waits. Those URLs name a run, a worktree, a plan or
// a primary checkout by id; the new ones name a branch by (project, branch) or
// an issue by (project, issue). The feed carries both halves, so this surface
// holds the screen for one snapshot, rewrites the hash to the work item the id
// belongs to, and gets out of the way. Nothing carries it → the inbox.
//
// Those URLs name one machine's work — the home device's, until a route can
// name its own — so the rows they are looked up in are that machine's: every
// machine mints a `proj-1`, and a primary checkout is named by its project
// alone.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { resolveLegacyRoute } from "../core/routeResolve.js";
import { homeFeedView } from "../core/deviceContexts.js";
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
    if (settled) return;
    settled = true;
    go(resolveLegacyRoute(reference, homeFeedView(feed).items) || { name: "inbox" });
  });
  App.viewDispose = unsubscribe;
}
