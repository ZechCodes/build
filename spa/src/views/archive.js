// The account archive: everything that has ended, across every project, on one
// reading page. Done archives an inbox entry; this is where the entry went.
//
// Reads `archived.list` — the archive is the user's, not a project's, so it is
// not a per-project pane any more. Opening a row states its record and offers
// nothing to do with it: an archived record is history.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { watchChanges } from "../core/changeEvents.js";
import { archiveListHtml, archiveRows } from "../core/archive.js";

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

  const load = async () => {
    let payload;
    try {
      payload = await App.call("archived.list");
    } catch {
      // A bridge too old to answer, or one that just went away. Whatever is
      // already on screen stays; a first read that fails says so.
      const host = $("#archive-list");
      if (!disposed && !painted && host) {
        host.innerHTML = '<div class="empty">The archive is unavailable right now. It will retry.</div>';
      }
      return;
    }
    if (disposed) return;
    rows = archiveRows(payload);
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
