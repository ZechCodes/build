// A page drawn as named parts, where a repaint replaces only the parts whose
// HTML changed.
//
// A surface that rewrites itself whole on every cache write stands up new
// nodes under the reader: a focused field loses its caret, and pictures whose
// bytes are a round trip away lose their height until they land, so the
// scroller clamps and then jumps (#153). Parts that did not change keep their
// nodes, so nothing the reader is looking at or typing into is touched.

/**
 * Patch `container`'s parts in place.
 *
 * `parts` is an ordered list of `{ name, html, key }`. A part is replaced when
 * its `key` (its `html` when none is given) differs from the one it was
 * painted with; `key` lets a part whose live state is updated in place — a
 * text field's value — say which changes need new nodes and which do not. A
 * part no longer listed is taken out, and a kept part out of place is moved.
 * `held` is the caller's record of what was painted, kept between calls.
 * `after` is the node the first part follows (null: the container's start).
 *
 * Answers the names of the parts that were painted anew.
 */
export function patchParts(container, held, parts, { after = null } = {}) {
  dropUnlisted(held, parts);
  const painted = [];
  let last = after;
  for (const part of parts) {
    if (repaintPart(container, held, part)) painted.push(part.name);
    const { nodes } = held.get(part.name);
    if (!follows(nodes[0], last, container)) {
      container.insertBefore(fragmentOf(container, nodes), last ? last.nextSibling : container.firstChild);
    }
    last = nodes.at(-1) || last;
  }
  return painted;
}

/** Stand one part up again when its key moved, off the page until it is
 *  placed. Answers whether it did. */
function repaintPart(container, held, { name, html, key = html }) {
  const was = held.get(name);
  if (was?.key === key) return false;
  was?.nodes.forEach((node) => node.remove());
  held.set(name, { key, nodes: nodesOf(container, html) });
  return true;
}

function dropUnlisted(held, parts) {
  const listed = new Set(parts.map((part) => part.name));
  for (const [name, { nodes }] of held) {
    if (listed.has(name)) continue;
    nodes.forEach((node) => node.remove());
    held.delete(name);
  }
}

/** Whether `node` already sits right after `last` (or first, with no `last`).
 *  A part stood up again is off the page, so it never does. */
const follows = (node, last, container) =>
  !node || (last ? last.nextSibling === node : container.firstChild === node);

function nodesOf(container, html) {
  const template = container.ownerDocument.createElement("template");
  template.innerHTML = html.trim();
  return [...template.content.childNodes];
}

function fragmentOf(container, nodes) {
  const fragment = container.ownerDocument.createDocumentFragment();
  fragment.append(...nodes);
  return fragment;
}
