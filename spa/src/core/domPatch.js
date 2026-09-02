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

// What is open on the page is the reader's answer, not the render's, and a
// poll tick must never shut it under the pointer reaching into it. The two
// things that open carry that fact in opposite directions, so the patch leaves
// each alone in the direction that would shut it.

/** `open` on a `<details>`: rendered shut — folded activity is folded on
 *  arrival — so taking the attribute back would shut every fold the reader had
 *  opened, once per poll. A render that says `open` still opens it. */
const readerOpenedFold = (live, name) => name === "open" && live.tagName === "DETAILS";

/** `hidden` on a `.splitmenu`: rendered shut too, and `mountSplitMenu` opens it
 *  by taking that attribute OFF — so re-applying what the render says is what
 *  would shut it. */
const readerOpenedMenu = (live, name) => name === "hidden" && live.classList.contains("splitmenu");

function patchAttributes(live, next) {
  // A picture the browser already loaded keeps the bytes it holds: the renderer
  // leaves `src` out when it does not know the bytes yet, never to take the
  // picture away. Only while it is the same attachment, though — an `<img>`
  // pointed at a new file must not go on showing the old one.
  const keepsItsBytes = sameAttachment(live, next);
  for (const { name, value } of [...next.attributes]) {
    if (readerOpenedMenu(live, name)) continue;
    if (live.getAttribute(name) !== value) live.setAttribute(name, value);
  }
  for (const { name } of [...live.attributes]) {
    if (next.hasAttribute(name)) continue;
    if (name === "src" && keepsItsBytes) continue;
    if (readerOpenedFold(live, name)) continue;
    live.removeAttribute(name);
  }
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
