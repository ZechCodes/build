// The account archive: everything that has ended, across every project and
// every machine, on one reading page. Done archives an inbox entry; this is
// where the entry went.
//
// Reads `archived.list` from every machine that can answer — the archive is the
// user's, not a project's and not a device's — and holds the merged list newest
// first. Opening a row states its record and offers nothing to do with it: an
// archived record is history, and a finished workspace has had its files and
// its live record removed, so the record is all there is to open.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { subscribeCache } from "../core/localCache.js";
import { liveContexts } from "../core/deviceContexts.js";
import { deviceKey } from "../core/deviceKey.js";
import { archiveDeviceNames, archiveListHtml, archiveRows, newestFirst } from "../core/archive.js";

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

export function renderArchive(options = {}) {
  const root = options.root || $("#root");
  root.innerHTML = `
    <div class="board-head"><div><h1>Archive</h1><p>Work you marked done, across every project.</p></div></div>
    <div id="archive-list"><div class="empty">Loading the archive…</div></div>`;

  let disposed = false;
  let rows = [];
  let openKey = null;
  let painted = false;
  let paintedFrom = null; // what the page currently stands on
  const archiveList = () => root.querySelector("#archive-list");

  const draw = () => {
    const host = archiveList();
    if (!host) return;
    // The archive is history, and the poll reads the same history over and over.
    // A rebuild would drop a selection someone is copying a path out of and the
    // focus they reached a card with, so an unchanged read leaves the page.
    const deviceNames = archiveDeviceNames(rows, App.devices);
    const source = JSON.stringify([rows, openKey, [...deviceNames]]);
    if (painted && source === paintedFrom) return;
    paintedFrom = source;
    host.innerHTML = archiveListHtml(rows, { openKey, deviceNames });
    host.querySelectorAll(".archive-row").forEach((card) => {
      const toggle = () => {
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
    const host = archiveList();
    if (!disposed && !painted && host) {
      host.innerHTML = '<div class="empty">The archive is unavailable right now. It will retry.</div>';
    }
  };

  const load = async () => {
    const answers = await Promise.all(liveContexts().map(readDeviceArchive));
    if (disposed || options.isCurrent?.() === false) return;
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

  let unwatch = null;
  const dispose = () => {
    disposed = true;
    if (unwatch) unwatch();
    unwatch = null;
  };
  if (options.registerDispose) options.registerDispose(dispose);
  else App.viewDispose = dispose;
  // The read is not awaited: the page (and the account nav above it) must be on
  // screen even when the device is unreachable and the read never lands.
  load();
  // Archiving is a lifecycle move, which is feed state: a machine's board
  // record moving is what says this list changed. The archive is not in the
  // cache-first brief and holds no records of its own, so this is the whole of
  // what wakes it — and a re-read that lands the same history repaints nothing
  // (`draw` above).
  unwatch = subscribeCache({}, (address) => {
    if (address.kind === "feed") void load();
  });
  if (!options.registerDispose) App.poll = { dispose };
}
