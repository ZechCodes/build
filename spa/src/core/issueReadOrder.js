// Which read last had the say on each issue of a list record (#85).
//
// A page of `issues.list` can be out while a newer read of the same list
// lands — a push re-read, another Issues tab's own read — and then lands
// after it, the older word. Its copy of a row can be older with the same
// `updated_at` (the bridge fills in who an issue's agent is when it lists it,
// and touches no timestamp), and a row it still names can already have left
// the list. So what decides is the order the reads were ASKED in: every read
// takes a number before it asks, and a page lays a row, or takes one away,
// only where no read asked after it has had the say on that row.
//
// A page has the say on a stretch of issue numbers, not only on the rows it
// names: every number in it that it does not name is not on the list. Numbers
// never move, so the stretch is kept, and a row a newer page left out stays
// out — the tombstone a timestamp cannot be, and one that holds for a row the
// cache never had. A card this tab moved itself has the say by its id.
//
// The same rule as `pushFence` (#142), per stretch rather than per record,
// since a page is the truth about a stretch of a list and not the whole of it.
// Held in memory: a read never outlives the tab that asked it. What another
// tab wrote is not in here; `trackerPages.js` falls back to the timestamps for
// that.

let asked = 0;
const ledgers = new Map(); // record address → { stretches, rows }

const addressKey = ({ deviceId, entityId, kind, sub = "" }) => JSON.stringify([deviceId, entityId, kind, sub]);

function ledgerOf(address) {
  const key = addressKey(address);
  if (!ledgers.has(key)) ledgers.set(key, { stretches: [], rows: new Map() });
  return ledgers.get(key);
}

const covers = (stretch, number) => number < stretch.above && number >= stretch.through;

/** The number a read takes before it asks: every later one is newer. */
export const nextIssueRead = () => ++asked;

/** The newest read that had the say on a row under this record, 0 for none:
 *  one whose stretch holds the row's number, or one that wrote it by id. */
export function lastSayOn(address) {
  const { stretches, rows } = ledgerOf(address);
  return (row) => stretches.reduce(
    (newest, stretch) => (covers(stretch, Number(row.number)) ? Math.max(newest, stretch.read) : newest),
    rows.get(row.id) || 0,
  );
}

/** A page asked as read `read` had the say on `through` (inclusive) up to
 *  `above` (exclusive) under this record. The last page of a pull that began
 *  as read `pullRead` completes it: every number has had a say at least that
 *  new, so every older say is spent and forgotten. */
export function noteStretch(address, { above, through, read, pullRead = read }) {
  const ledger = ledgerOf(address);
  if (through === -Infinity) forgetOlder(ledger, pullRead);
  ledger.stretches.push({ above, through, read });
}

function forgetOlder(ledger, read) {
  ledger.stretches = ledger.stretches.filter((stretch) => stretch.read >= read);
  for (const [issueId, said] of ledger.rows) if (said < read) ledger.rows.delete(issueId);
}

/** This tab wrote these issues under these records itself — a card moved
 *  before the bridge said so — which is newer than any read already out. */
export function noteWritten(addresses, issueIds) {
  const read = nextIssueRead();
  for (const address of addresses) {
    const { rows } = ledgerOf(address);
    for (const issueId of issueIds) rows.set(issueId, read);
  }
}
