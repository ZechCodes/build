// Cache repaints of an open menu keep its nodes, focus, motion and listeners.
// Compare the two renderings so runtime attributes (expanded, lifted styles)
// and slider previews are never mistaken for stale markup.
import { updateMenuSlider } from "./menuSlider.js";

function renderedTree(markup) {
  const template = document.createElement("template");
  template.innerHTML = markup;
  return template.content;
}

function sameRow(before, after) {
  if (before.getAttribute("data-group") !== after.getAttribute("data-group")) return false;
  if (before.matches(".mi") && before.dataset.action !== after.dataset.action) return false;
  return before.matches(".menu-slider") === after.matches(".menu-slider");
}

function sameStructure(before, after) {
  if (before.nodeName !== after.nodeName) return false;
  if (before.nodeType === Node.ELEMENT_NODE) {
    if (!sameRow(before, after)) return false;
    // A slider's stops can change without replacing the input or its control.
    if (before.matches(".menu-slider")) return true;
    if (!before.childElementCount && !after.childElementCount) return true;
  }
  return before.childNodes.length === after.childNodes.length &&
    [...before.childNodes].every((child, index) => sameStructure(child, after.childNodes[index]));
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

function patchNode(live, before, after) {
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
  [...before.childNodes].forEach((child, index) => patchNode(live.childNodes[index], child, after.childNodes[index]));
}

/** Returns false before changing anything when rows were added, removed or
 * reordered. Closed menus and structural changes use the usual remount. */
export function patchSplitMenu(container, beforeMarkup, afterMarkup) {
  const before = renderedTree(beforeMarkup);
  const after = renderedTree(afterMarkup);
  if (!sameStructure(before, after)) return false;
  [...before.childNodes].forEach((child, index) => patchNode(container.childNodes[index], child, after.childNodes[index]));
  return true;
}
