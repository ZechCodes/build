// Making a live list say what a freshly rendered one says, moving as little of
// it as possible.
//
// `patchElement` already keeps one element's subtree in place, but a list is
// not one subtree: entries arrive, leave and change places, and walking two
// lists position by position makes the second entry answer for the first the
// moment anything is inserted at the top. Everything after the insert then
// redraws — which is the whole list, on a surface that re-renders on a poll.
//
// So entries are matched by key rather than by position. An entry that is still
// there is still the same element afterwards, whatever else moved around it: it
// keeps the reply half typed into it, the caret in that reply, the row menu the
// reader left open, the picture it had already fetched, and the place the
// browser's scroll anchoring was holding. Only new, updated and removed entries
// redraw.
//
// ## The contract
//
// - `keyOf(entry)` names the entry, and the name is stamped onto its element as
//   `data-key`. Keys are read back as strings and must be unique in a paint.
// - `render(entry)` gives back one element, or the markup for one. It is called
//   for every entry on every paint; what it says is compared against the live
//   element, and only the disagreements are written.
// - `wire(element, entry)` runs once for each element that had to be made, and
//   never on a patch — so handlers attach exactly once and survive every later
//   paint. It runs in list order, after the paint has settled, so the element
//   is standing in its finished list. The entry it is handed is the entry of
//   that moment; a handler that outlives it should look the current one up by
//   key, which is the shape views already use (`row.dataset.key` → find the
//   entry → act).
// - Patching never replaces a kept entry's element. The one thing that does is
//   a render that changed the entry's tag name — a different kind of element
//   cannot be patched into the old one, so that entry is made again, in its
//   right place, and wired again with it.
// - The container's keyed children are patchList's; every other child is left
//   where it is. Headers, empty states and footers are the caller's, and belong
//   either ahead of the entries or outside the container: new entries are
//   appended at the end of a container that holds no entries yet.

import { patchElement } from "./domPatch.js";

const KEY = "data-key";
const ELEMENT_NODE = 1;

/** The one element the render described, ready to be patched in or inserted. */
function elementFrom(rendered, page) {
  if (typeof rendered !== "string") return rendered;
  const holder = page.createElement("template");
  holder.innerHTML = rendered;
  return holder.content.firstElementChild;
}

function renderEntry(render, entry, key, page) {
  const element = elementFrom(render(entry), page);
  if (!element || element.nodeType !== ELEMENT_NODE) {
    throw new Error(`patchList: the render of entry "${key}" gave back no element`);
  }
  if (element.getAttribute(KEY) !== key) element.setAttribute(KEY, key);
  return element;
}

/// Put `element` in front of `anchor`, keeping whatever the browser will let it
/// keep.
///
/// Taking a node out and putting it back is what costs a moved entry its focus,
/// and `moveBefore` is the browser saying it can move one without that. Where
/// it is missing the move is still a move — the element, and everything typed
/// into it, is the same element on the other side.
function place(container, element, anchor) {
  const moves = element.parentNode === container && container.isConnected;
  if (moves && typeof container.moveBefore === "function") {
    container.moveBefore(element, anchor);
    return;
  }
  container.insertBefore(element, anchor);
}

/// Which of the entries are already in the right order relative to each other.
///
/// Everything else has to move; these can stay. Reading the longest run of
/// entries whose live positions ascend is what makes a list of ten with one
/// entry pulled to the front cost one move instead of nine — and reversing a
/// list cost the honest count rather than a rebuild.
///
/// `positions` holds each wanted entry's place in the live list, or -1 for one
/// that is not there yet.
function alreadyInOrder(positions) {
  const runEnds = []; // runEnds[n]: the entry ending the shallowest ascending run of length n + 1.
  const before = new Array(positions.length).fill(-1);
  positions.forEach((position, index) => {
    if (position < 0) return;
    let low = 0;
    let high = runEnds.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (positions[runEnds[middle]] < position) low = middle + 1;
      else high = middle;
    }
    before[index] = low > 0 ? runEnds[low - 1] : -1;
    runEnds[low] = index;
  });
  const staying = new Set();
  let index = runEnds.length ? runEnds[runEnds.length - 1] : -1;
  while (index >= 0) {
    staying.add(index);
    index = before[index];
  }
  return staying;
}

/// The keyed children `container` gets to keep, in the order they sit in.
///
/// Everything else keyed goes: the entries that left, and any second element
/// claiming a key already spoken for — a ghost the paint would otherwise never
/// be able to reach again, since a key only ever finds the first of them.
function keptEntries(container, wanted) {
  const live = new Map();
  for (const child of [...container.children]) {
    const key = child.getAttribute(KEY);
    if (key === null) continue;
    if (!wanted.has(key) || live.has(key)) {
      container.removeChild(child);
      continue;
    }
    live.set(key, child);
  }
  return live;
}

function keysWanted(entries, keyOf) {
  const keys = entries.map((entry) => String(keyOf(entry)));
  const seen = new Set();
  for (const key of keys) {
    if (seen.has(key)) throw new Error(`patchList: two entries claim the same key "${key}"`);
    seen.add(key);
  }
  return keys;
}

/** Where the entries end and the caller's trailing chrome begins. */
function endOfEntries(container) {
  let last = null;
  for (const child of container.children) {
    if (child.getAttribute(KEY) !== null) last = child;
  }
  return last ? last.nextSibling : null;
}

export function rekeyEntry(container, fromKey, toKey) {
  const from = String(fromKey);
  const to = String(toKey);
  const standing = [...container.children].find((child) => child.getAttribute(KEY) === from);
  if (!standing) return false;
  if ([...container.children].some((child) => child !== standing && child.getAttribute(KEY) === to)) {
    container.removeChild(standing);
    return false;
  }
  standing.setAttribute(KEY, to);
  return true;
}

/// Make the keyed children of `container` say what `entries` says.
///
/// Returns the entry elements, in order. See the contract at the top of this
/// module: identical entries come out of a paint untouched, kept entries keep
/// their element, and `wire` runs only for the ones that had to be made.
export function patchList(container, entries, { keyOf, render, wire }) {
  const page = container.ownerDocument;
  const keys = keysWanted(entries, keyOf);
  const live = keptEntries(container, new Set(keys));

  const positions = new Map([...live.keys()].map((key, index) => [key, index]));
  const staying = alreadyInOrder(keys.map((key) => (positions.has(key) ? positions.get(key) : -1)));

  // Backwards, so each entry is placed in front of the one that is to follow
  // it — which is by then already where it belongs.
  const painted = new Array(entries.length);
  const made = [];
  let anchor = endOfEntries(container);
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    const key = keys[index];
    const standing = live.get(key);
    const next = renderEntry(render, entry, key, page);
    if (standing && standing.tagName === next.tagName) {
      patchElement(standing, next);
      if (!staying.has(index)) place(container, standing, anchor);
      anchor = standing;
      painted[index] = standing;
      continue;
    }
    // Either the entry is new, or the render made it a different kind of
    // element than the one standing there — which cannot be patched into it. So
    // the element is made, and wired, for the first time.
    if (standing) container.removeChild(standing);
    container.insertBefore(next, anchor);
    made.unshift({ element: next, entry });
    anchor = next;
    painted[index] = next;
  }

  // Once, in list order, with the list already saying what it will say — so a
  // handler can measure the row it is on or scroll it into view.
  if (wire) for (const { element, entry } of made) wire(element, entry);
  return painted;
}
