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
import { subscribeBoardWrites } from "../core/feedRows.js";
import { liveContexts } from "../core/deviceContexts.js";
import { deviceKey } from "../core/deviceKey.js";
import { archiveDeviceNames, archiveListHtml, archiveRows, newestFirst } from "../core/archive.js";
import { readCachedMany, subscribeCache, writeCached } from "../core/localCache.js";

export const archiveAddress = (deviceId) => ({ deviceId, entityId: "", kind: "archive" });

/** A bridge mints its own record ids, so every cached slice keeps its device. */
const rowsFor = (deviceId, payload) =>
  archiveRows(payload).map((row) => ({ ...row, deviceId, key: deviceKey(deviceId, row.key) }));

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
  let readGeneration = 0;
  const cacheWrites = new Map();
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

  const readArchive = async () => {
    const generation = ++readGeneration;
    const ids = [...new Set([...App.devices.map((device) => device.id), ...liveContexts().map((context) => context.deviceId)])];
    const records = await readCachedMany(ids.map(archiveAddress));
    if (disposed || options.isCurrent?.() === false || generation !== readGeneration) return;
    const held = records.map((record, index) => record ? rowsFor(ids[index], record.value) : null).filter(Boolean);
    if (!held.length && !painted) return;
    rows = held.flat().sort(newestFirst);
    // A row that vanished cannot stay open under a row it no longer is.
    if (openKey && !rows.some((row) => row.key === openKey)) openKey = null;
    draw();
  };

  const load = async () => {
    if (disposed || options.isCurrent?.() === false) return;
    const answers = await Promise.all(liveContexts().map(async (context) => {
      const revision = cacheWrites.get(context.deviceId) || 0;
      try {
        const payload = await context.call("archived.list");
        if (disposed || options.isCurrent?.() === false || !context.active()) return false;
        // A cache writer that landed while the pull was out has newer news.
        if ((cacheWrites.get(context.deviceId) || 0) !== revision) return true;
        await writeCached(archiveAddress(context.deviceId), payload);
        return true;
      } catch {
        return false;
      }
    }));
    if (!disposed && !answers.some(Boolean)) sayUnavailable();
  };

  const unwatchArchive = subscribeCache({}, (address) => {
    if (address.kind === "archive" && address.deviceId) {
      cacheWrites.set(address.deviceId, (cacheWrites.get(address.deviceId) || 0) + 1);
      void readArchive();
    } else if (address.kind === undefined) {
      void readArchive();
    }
  });
  const unwatchBoard = subscribeBoardWrites(load);
  const dispose = () => {
    disposed = true;
    unwatchArchive();
    unwatchBoard();
  };
  if (options.registerDispose) options.registerDispose(dispose);
  else App.viewDispose = dispose;
  // The read is not awaited: the page (and the account nav above it) must be on
  // screen even when the device is unreachable and the read never lands.
  void readArchive().then(load);
  // Archiving is a lifecycle move, which is feed state: a machine's board
  // moving asks for fresh archive records. Each answer writes its device's
  // record, whose announcement re-reads the cache before drawing.
  //
  // The read itself is handed over rather than fired and forgotten: a pass
  // writes a board and every row on it, and what keeps that from being a read
  // per row is the wake knowing when this one is still out (core/feedRows.js).
  if (!options.registerDispose) App.poll = { dispose };
}
