// A merge of already cached ordered heads, with no RPC and no automatic page
// traversal. Unknown/empty sparse heads form a barrier: another stream cannot
// prove it owns the next globally ordered item until all heads are known.
const position = (stream, consumed) => consumed[stream.id] || 0;
const head = (stream, consumed) => stream.items[position(stream, consumed)];
const unknownHeads = (streams, consumed) => streams.filter((stream) => !head(stream, consumed) && !stream.complete).map((stream) => stream.id);

function nextStream(streams, consumed, compare) {
  const candidates = streams.filter((stream) => head(stream, consumed));
  candidates.sort((left, right) => {
    const a = head(left, consumed), b = head(right, consumed);
    // Stable sort retains source stream order for exact ties, matching Inbox.
    return compare(a, b);
  });
  return candidates[0];
}

/** Per-stream `complete` means no further pages after its current buffer,
 * never merely a short/empty page. Comparator must match the server order.
 * `seen` and consumed positions belong to the caller's captured query basis. */
export function mergeKnownTaskHeads(streams, compare, limit, previous = new Set()) {
  const consumed = Object.fromEntries(streams.map((stream) => [stream.id, 0]));
  const seen = new Set(previous);
  const items = [];
  while (items.length < limit) {
    if (unknownHeads(streams, consumed).length) break;
    const stream = nextStream(streams, consumed, compare);
    if (!stream) break;
    const item = head(stream, consumed);
    consumed[stream.id] += 1;
    if (seen.has(item.id)) continue;
    seen.add(item.id);
    items.push(item);
  }
  return { items, consumed, seen, needsHeads: unknownHeads(streams, consumed),
    complete: streams.every((stream) => stream.complete && position(stream, consumed) === stream.items.length) };
}

/** Admission plan only: even a thousand unknown project heads admit at most
 * this operation's explicit request budget. Unadmitted heads remain unknown. */
export const taskHeadReadPlan = (streams, maxRequests) =>
  streams.filter((stream) => !stream.items.length && !stream.complete).slice(0, Math.max(0, Math.min(4, maxRequests))).map((stream) => stream.id);
