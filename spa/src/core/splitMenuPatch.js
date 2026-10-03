// Cache repaints of an open menu keep its nodes, focus, motion and listeners.
// Compare the two renderings so runtime attributes (expanded, lifted styles)
// and slider previews are never mistaken for stale markup.
import { MENU_SLIDER_SELECTOR, disposeMenuSlider, updateMenuSlider } from "./menuSlider.js";

const FOCUS_ROWS = `.mi, ${MENU_SLIDER_SELECTOR}`;

function renderedTree(markup) {
  const template = document.createElement("template");
  template.innerHTML = markup;
  return template.content;
}

function nodeKey(node, index) {
  if (node.nodeType !== Node.ELEMENT_NODE) return `${node.nodeName}:${index}`;
  let key = node.classList[0] || index;
  if (node.hasAttribute("data-group")) key = `group:${node.dataset.group}`;
  if (node.hasAttribute("data-action")) key = `action:${node.dataset.action}`;
  if (node.matches(".caret")) key = "caret";
  return `${node.nodeName}:${key}`;
}

function keyedChildren(parent) {
  const counts = new Map();
  return [...parent.childNodes].map((node, index) => {
    const base = nodeKey(node, index);
    const occurrence = counts.get(base) || 0;
    counts.set(base, occurrence + 1);
    return { node, index, key: `${base}:${occurrence}` };
  });
}

function adjacentKeptNode(node, direction, kept) {
  // A retired sibling must not force a surviving row to move before cleanup.
  let sibling = node[direction];
  while (sibling && !kept.has(sibling)) sibling = sibling[direction];
  return sibling;
}

function placeBefore(parent, nodes, anchor, kept) {
  for (const node of [...nodes].reverse()) {
    if (node.parentNode !== parent || adjacentKeptNode(node, "nextSibling", kept) !== anchor) parent.insertBefore(node, anchor);
    anchor = node;
  }
}

function placeAfter(parent, nodes, anchor, kept) {
  for (const node of nodes) {
    if (node.parentNode !== parent || adjacentKeptNode(node, "previousSibling", kept) !== anchor) parent.insertBefore(node, anchor.nextSibling);
    anchor = node;
  }
}

function orderChildren(parent, nodes) {
  const kept = new Set(nodes);
  const focused = nodes.find((node) => node.contains(document.activeElement));
  if (!focused) {
    placeBefore(parent, nodes, null, kept);
    return;
  }
  // Moving a focused node (or its ancestor) with insertBefore blurs it.
  // Reorder siblings around that fixed node instead, keeping focus untouched.
  const index = nodes.indexOf(focused);
  placeBefore(parent, nodes.slice(0, index), focused, kept);
  placeAfter(parent, nodes.slice(index + 1), focused, kept);
}

function patchChildren(live, before, after, removed) {
  const previous = new Map(keyedChildren(before).map(({ node, index, key }) =>
    [key, { rendered: node, live: live.childNodes[index] }]));
  const next = keyedChildren(after).map(({ node, key }) => {
    const standing = previous.get(key);
    if (!standing) return node.cloneNode(true);
    previous.delete(key);
    patchNode(standing.live, standing.rendered, node, removed);
    return standing.live;
  });
  orderChildren(live, next);
  previous.forEach((node) => removed.push(node.live));
}

function patchAttributes(live, before, after) {
  const names = new Set([...before.getAttributeNames(), ...after.getAttributeNames()]);
  for (const name of names) {
    const value = after.getAttribute(name);
    if (before.getAttribute(name) === value) continue;
    if (value === null) live.removeAttribute(name);
    else live.setAttribute(name, value);
  }
}

function patchNode(live, before, after, removed) {
  if (before.nodeType === Node.ELEMENT_NODE) {
    if (before.matches(".menu-slider")) {
      updateMenuSlider(live, after);
      return;
    }
    patchAttributes(live, before, after);
    if (!before.childElementCount && !after.childElementCount) {
      if (before.textContent !== after.textContent) live.textContent = after.textContent;
      return;
    }
  } else if (before.nodeValue !== after.nodeValue) {
    live.nodeValue = after.nodeValue;
  }
  patchChildren(live, before, after, removed);
}

function nearestRow(beforeRows, afterRows, focused) {
  const index = beforeRows.indexOf(focused);
  const next = beforeRows.slice(index + 1).find((row) => afterRows.includes(row));
  const previous = beforeRows.slice(0, index).reverse().find((row) => afterRows.includes(row));
  if (!next) return previous || afterRows[Math.min(index, afterRows.length - 1)];
  if (!previous) return next;
  return beforeRows.indexOf(next) - index <= index - beforeRows.indexOf(previous) ? next : previous;
}

function keepFocusBeforeRemoval(container, beforeRows, removed) {
  const focused = document.activeElement;
  if (!removed.some((node) => node.contains(focused))) return;
  const afterRows = [...container.querySelectorAll(FOCUS_ROWS)]
    .filter((row) => !removed.some((node) => node.contains(row)));
  const destination = nearestRow(beforeRows, afterRows, focused) || container.querySelector(".caret");
  destination?.focus({ preventScroll: true });
}

function retireMenuSliders(node) {
  if (node.nodeType !== Node.ELEMENT_NODE) return;
  if (node.matches(MENU_SLIDER_SELECTOR)) disposeMenuSlider(node);
  node.querySelectorAll(MENU_SLIDER_SELECTOR).forEach(disposeMenuSlider);
}

/** Reconcile keyed groups and rows, including insertions and removals. Only
 * retired rows leave the DOM; the open menu and surviving focus stay put. */
export function patchSplitMenu(container, beforeMarkup, afterMarkup) {
  const before = renderedTree(beforeMarkup);
  const after = renderedTree(afterMarkup);
  const beforeRows = [...container.querySelectorAll(FOCUS_ROWS)];
  const removed = [];
  patchChildren(container, before, after, removed);
  removed.forEach(retireMenuSliders);
  // Insert destinations first, then hand focus across before retiring a row.
  keepFocusBeforeRemoval(container, beforeRows, removed);
  removed.forEach((node) => node.remove());
}
