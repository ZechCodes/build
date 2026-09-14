// The account archive: everything that has ended, across every project and
// every machine, on one reading page. Done archives an inbox entry; this is
// where the entry went.
//
// Reads `archived.list` from every machine that can answer — the archive is the
// user's, not a project's and not a device's — and holds the merged list newest
// first. Opening a row states its record and offers nothing to do with it: an
// archived record is history.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { watchChanges } from "../core/changeEvents.js";
import { liveContexts } from "../core/deviceContexts.js";
import { deviceKey } from "../core/deviceKey.js";
import { archiveListHtml, archiveRows, newestFirst } from "../core/archive.js";

/** One machine's share of the archive: its rows, each stamped with the machine
 *  that answered for it and keyed by it, since every daemon mints its own
 *  record ids and two machines can hand back the same one. A machine that will
 *  not answer contributes nothing rather than emptying the page. */
const readDeviceArchive = (context) =>
  context
    .call("archived.list")
    .then((payload) =>
      archiveRows(payload).map((row) => ({ ...row, deviceId: context.deviceId, key: deviceKey(context.deviceId, row.key) })),
    )
    .catch(() => null);

const POLL_MS = 15000;

export function renderArchive() {
  const root = $("#root");
  root.innerHTML = `
    <div class="board-head"><div><h1>Archive</h1><p>Work you marked done, across every project.</p></div></div>
    <div id="archive-list"><div class="empty">Loading the archive…</div></div>`;

  let disposed = false;
  let rows = [];
  let openKey = null;
  let painted = false;
  let paintedFrom = null; // what the page currently stands on

  const draw = () => {
    const host = $("#archive-list");
    if (!host) return;
    // The archive is history, and the poll reads the same history over and over.
    // A rebuild would drop a selection someone is copying a path out of and the
    // focus they reached a card with, so an unchanged read leaves the page.
    const source = JSON.stringify([rows, openKey]);
    if (painted && source === paintedFrom) return;
    paintedFrom = source;
    host.innerHTML = archiveListHtml(rows, { openKey });
    host.querySelectorAll(".archive-row").forEach((card) => {
      const toggle = () => {
        const row = rows.find((candidate) => candidate.key === card.dataset.key);
        if (row?.kind === "workspace" && row.workspaceId && row.projectId) {
          go({ name: "workspace", projectId: row.projectId, workspaceId: row.workspaceId, tab: "changes" });
          return;
        }
        openKey = openKey === card.dataset.key ? null : card.dataset.key;
        draw();
      };
      card.onclick = toggle;
      card.onkeydown = (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        toggle();
      };
    });
    painted = true;
  };

  // Nothing answered: a bridge too old to have the method, one that just went
  // away, or an account with no machine on it right now. Whatever is already on
  // screen stays; a first read that lands nothing says so.
  const sayUnavailable = () => {
    const host = $("#archive-list");
    if (!disposed && !painted && host) {
      host.innerHTML = '<div class="empty">The archive is unavailable right now. It will retry.</div>';
    }
  };

  const load = async () => {
    const answers = await Promise.all(liveContexts().map(readDeviceArchive));
    if (disposed) return;
    const landed = answers.filter((answer) => answer !== null);
    if (!landed.length) {
      sayUnavailable();
      return;
    }
    rows = landed.flat().sort(newestFirst);
    // A row that vanished cannot stay open under a row it no longer is.
    if (openKey && !rows.some((row) => row.key === openKey)) openKey = null;
    draw();
  };

  let watcher = null;
  App.viewDispose = () => {
    disposed = true;
    if (watcher) watcher.dispose();
    watcher = null;
  };
  // The read is not awaited: the page (and the account nav above it) must be on
  // screen even when the device is unreachable and the read never lands.
  load();
  // Archiving is a lifecycle move, which is feed state: the board's own event
  // is what says this list changed.
  watcher = watchChanges({ refresh: load, intervalMs: POLL_MS });
  App.poll = watcher;
}
