// Making a live subtree say what a freshly rendered one says, moving as little
// of it as possible.
//
// Every conversation surface re-renders its whole timeline on a 1.6s poll, and
// writing that markup in with innerHTML replaces every node under it even when
// one word changed. The nodes that come back are not the nodes that left, and
// the reader pays for the difference: a selection collapses, an inline picture
// loses the bytes it had fetched and re-lays-out at zero height, and the
// browser's scroll anchoring — which keeps a node still, not a number — is
// handed a subtree with no still nodes in it and drags the viewport. So the two
// trees are walked together and only the disagreements are written.

const ELEMENT_NODE = 1;

const MOVED_PROPERTIES = ["hidden", "style"];

export const EXPANDED_ATTRIBUTE = "data-expanded";
export const KEYED_LIST_ATTRIBUTE = "data-keyed-list";

/** Whether two nodes can be made to say the same thing, or one has to replace
 *  the other outright. */
const interchangeable = (live, next) =>
  live.nodeType === next.nodeType &&
  (live.nodeType !== ELEMENT_NODE || live.tagName === next.tagName);

/** Whether these two `<img>`s are the same picture — read BEFORE the
 *  attributes are synced, since syncing is what would make any two of them
 *  look alike. */
const sameAttachment = (live, next) =>
  live.tagName === "IMG" &&
  live.getAttribute("data-attachment-path") === next.getAttribute("data-attachment-path");

/** Marks a `<details>` whose renderer owns the fold outright — see
 *  `foldTheRenderOwns`. */
export const RENDERED_FOLD_ATTRIBUTE = "data-rendered-fold";

/** Whether the render, rather than the reader, is what says this fold is open.
 *  A run of activity is one such fold: what it holds is fetched on the press
 *  and drawn on the repaint, so the render is its only writer and shutting one
 *  means taking `open` back off. */
const foldTheRenderOwns = (live) => live.hasAttribute(RENDERED_FOLD_ATTRIBUTE);

const foldTheReaderOpened = (live, name) =>
  name === "open" && live.tagName === "DETAILS" && !foldTheRenderOwns(live);

const EXPANSION_MARKS = [EXPANDED_ATTRIBUTE, "aria-expanded"];

const CLIPPED_LINE_ELEMENT = ".surface-clip";

const expansionTheReaderOwns = (live, name) =>
  EXPANSION_MARKS.includes(name) && live.hasAttribute(EXPANDED_ATTRIBUTE) && live.matches(CLIPPED_LINE_ELEMENT);

const keyedByAnotherPainter = (live) => live.hasAttribute(KEYED_LIST_ATTRIBUTE);

const menuTheReaderOpened = (live, name) => name === "hidden" && live.classList.contains("splitmenu");

const shownByAMove = (live, name) => MOVED_PROPERTIES.includes(name) && live.hasAttribute("data-motion");

// eslint-disable-next-line complexity -- ratchet: patchAttributes is at 16, cap 10 — reduce it, then drop this line
function patchAttributes(live, next) {
  // A picture the browser already loaded keeps the bytes it holds: the renderer
  // leaves `src` out when it does not know the bytes yet, never to take the
  // picture away. Only while it is the same attachment, though — an `<img>`
  // pointed at a new file must not go on showing the old one.
  const keepsItsBytes = sameAttachment(live, next);
  const keepsItsSurface = live.tagName === "CANVAS";
  for (const { name, value } of [...next.attributes]) {
    if (menuTheReaderOpened(live, name)) continue;
    if (expansionTheReaderOwns(live, name)) continue;
    if (shownByAMove(live, name)) continue;
    if (live.getAttribute(name) !== value) live.setAttribute(name, value);
  }
  for (const { name } of [...live.attributes]) {
    if (next.hasAttribute(name)) continue;
    if (name === "src" && keepsItsBytes) continue;
    if ((name === "width" || name === "height") && keepsItsSurface) continue;
    if (foldTheReaderOpened(live, name)) continue;
    if (expansionTheReaderOwns(live, name)) continue;
    if (shownByAMove(live, name)) continue;
    live.removeAttribute(name);
  }
}

/** Whether this element's ticked-ness is state the render declares. */
const boxTheRenderOwns = (live) =>
  live.tagName === "INPUT" && (live.type === "checkbox" || live.type === "radio");

/// Make a box say what the render says about it.
///
/// Pressing a checkbox sets its `checked` PROPERTY; the render declares it as
/// an ATTRIBUTE. Comparing attributes alone, a box the reader ticked and a
/// render saying "unticked" already agree — so nothing was written and the box
/// stayed ticked on screen while the state behind it said otherwise.
///
/// Only boxes. What is being typed into a text field is the reader's, not the
/// render's, and syncing `value` the same way would take a half-written
/// sentence away mid-keystroke.
function patchCheckedness(live, next) {
  if (!boxTheRenderOwns(live)) return;
  const declared = next.hasAttribute("checked");
  if (live.checked !== declared) live.checked = declared;
}

/// Make `live` say what `next` says.
///
/// `next` is a throwaway tree parsed from the render, and the nodes it still
/// needs are moved out of it rather than copied. Two trees that already agree
/// come out of this untouched — no attribute set to the value it holds, no
/// child detached and re-attached — which is what makes a poll tick that
/// resolved the same conversation a true no-op.
export function patchElement(live, next) {
  patchAttributes(live, next);
  patchCheckedness(live, next);
  if (keyedByAnotherPainter(live)) return;
  const liveChildren = [...live.childNodes];
  const nextChildren = [...next.childNodes];
  nextChildren.forEach((source, index) => {
    const target = liveChildren[index];
    if (!target) {
      live.appendChild(source);
      return;
    }
    if (!interchangeable(target, source)) {
      live.replaceChild(source, target);
      return;
    }
    if (target.nodeType === ELEMENT_NODE) {
      patchElement(target, source);
      return;
    }
    if (target.nodeValue !== source.nodeValue) target.nodeValue = source.nodeValue;
  });
  for (const extra of liveChildren.slice(nextChildren.length)) live.removeChild(extra);
}

export function patchInnerHtml(host, html) {
  const next = host.cloneNode(false);
  next.innerHTML = html;
  patchElement(host, next);
}
