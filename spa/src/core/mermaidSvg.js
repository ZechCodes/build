// Markdown was escape-first, not DOM-sanitized. Mermaid introduces a second
// boundary: rebuild a small SVG vocabulary, never insert its returned markup.
// In particular NO CSS, authored classes, HTML, links, images or external URLs.
const NS = "http://www.w3.org/2000/svg";
const TAGS = new Set("svg g defs marker clipPath path rect circle ellipse line polyline polygon text tspan title desc".split(" "));
const NUMBER = "[+-]?(?:\\d+\\.?\\d*|\\.\\d+)(?:[eE][+-]?\\d+)?";
const NUMBERS = new RegExp(`^(?:${NUMBER}(?:em|px|%)?[, ]*)+$`);
const GEOMETRY = new Set("x y x1 y1 x2 y2 cx cy r rx ry width height dx dy refX refY markerWidth markerHeight stroke-width stroke-dasharray stroke-dashoffset font-size textLength lengthAdjust".split(" "));
const ENUMS = {
  "text-anchor": /^(start|middle|end)$/,
  "dominant-baseline": /^(auto|middle|central|hanging|text-before-edge|text-after-edge)$/,
  "markerUnits": /^(strokeWidth|userSpaceOnUse)$/,
  "orient": new RegExp(`^(?:auto|auto-start-reverse|${NUMBER})$`),
  "preserveAspectRatio": /^x(?:Min|Mid|Max)Y(?:Min|Mid|Max)(?: (?:meet|slice))?$/,
};
const LOCAL_REFERENCE = new Set(["marker-start", "marker-mid", "marker-end", "clip-path"]);

function safeGeometry(name, value) {
  if (GEOMETRY.has(name)) return NUMBERS.test(value);
  if (name === "viewBox") return new RegExp(`^${NUMBER}[ ,]+${NUMBER}[ ,]+${NUMBER}[ ,]+${NUMBER}$`).test(value);
  if (name === "d") return /^[MmZzLlHhVvCcSsQqTtAaEe0-9+.,\s-]+$/.test(value);
  if (name === "points") return NUMBERS.test(value);
  if (name === "transform") return /^(?:(?:matrix|translate|scale|rotate|skewX|skewY)\([0-9eE+.,\s-]+\)\s*)+$/.test(value);
  return Boolean(ENUMS[name]?.test(value));
}

function uniqueIds(root, prefix) {
  const ids = new Map();
  for (const element of [root, ...root.querySelectorAll("[id]")]) {
    const id = element.getAttribute("id");
    if (!id || !TAGS.has(element.localName)) continue;
    ids.set(id, ids.has(id) ? null : `${prefix}-${ids.size}`);
  }
  return ids;
}

function copyAttribute(target, attribute, ids) {
  const { name, value } = attribute;
  if (attribute.namespaceURI) return;
  if (name === "id" && ids.get(value)) target.setAttribute("id", ids.get(value));
  else if (LOCAL_REFERENCE.has(name)) {
    const id = /^url\(#([^\s()]+)\)$/.exec(value)?.[1];
    if (ids.get(id)) target.setAttribute(name, `url(#${ids.get(id)})`);
  } else if (name === "fill" && value === "none") target.setAttribute(name, "none");
  else if (safeGeometry(name, value)) target.setAttribute(name, value);
}

function copyPresentation(element, clean) {
  // Convert only individually validated presentation values into SVG attrs;
  // stylesheets and the original style attribute are always discarded.
  for (const declaration of (element.getAttribute("style") || "").split(";")) {
    const [name, value] = declaration.split(":").map((part) => part.trim());
    if (["text-anchor", "dominant-baseline", "font-size"].includes(name) && safeGeometry(name, value || "")) clean.setAttribute(name, value);
    if (name === "fill" && value === "none") clean.setAttribute("fill", "none");
  }
}

function presentation(element, clean, semantics) {
  copyPresentation(element, clean);
  const own = element.getAttribute("class") || "";
  if (/(?:^|\s)(?:messageLine1|edge-pattern-dotted|edge-pattern-dashed)(?:$|\s)/.test(own)) clean.setAttribute("stroke-dasharray", "3 3");
  const semantic = semantics.get(element) || (/^md-svg-(?:slice-[0-7]|shape|dot|title|message-solid|message-dashed|marker-hollow|marker-filled)$/.test(own) ? own : null);
  if (semantic) clean.setAttribute("class", semantic);
}

function semanticShapes(root) {
  const shapes = new Map();
  root.querySelectorAll(".pieCircle").forEach((element, index) => shapes.set(element, `md-svg-slice-${index % 8}`));
  root.querySelectorAll(".legend > rect").forEach((element, index) => shapes.set(element, `md-svg-slice-${index % 8}`));
  root.querySelectorAll("path.label-container").forEach((element) => shapes.set(element, "md-svg-shape"));
  root.querySelectorAll(".messageLine0").forEach((element) => shapes.set(element, "md-svg-message-solid"));
  root.querySelectorAll(".messageLine1").forEach((element) => shapes.set(element, "md-svg-message-dashed"));
  root.querySelectorAll(".pieTitleText").forEach((element) => shapes.set(element, "md-svg-title"));
  root.querySelectorAll("marker").forEach((element) => {
    if (/\b(?:aggregation|extension|lollipop)\b/.test(element.getAttribute("class") || "")) shapes.set(element, "md-svg-marker-hollow");
    if (element.classList.contains("composition")) shapes.set(element, "md-svg-marker-filled");
  });
  root.querySelectorAll("circle:not(marker circle)").forEach((element) => {
    const radius = Number(element.getAttribute("r"));
    if (radius > 0 && radius <= 10) shapes.set(element, "md-svg-dot");
  });
  return shapes;
}

function textChild(parent, child) {
  return child.nodeType === 3 && ["text", "tspan", "title", "desc"].includes(parent.localName);
}

function rebuild(element, doc, ids, semantics, depth = 0) {
  if (depth > 40) throw new Error("Diagram is too deep");
  if (element.namespaceURI !== NS || !TAGS.has(element.localName)) return null;
  const clean = doc.createElementNS(NS, element.localName);
  for (const attribute of element.attributes) copyAttribute(clean, attribute, ids);
  presentation(element, clean, semantics);
  for (const child of element.childNodes) {
    if (child.nodeType === 1) {
      const rebuilt = rebuild(child, doc, ids, semantics, depth + 1);
      if (rebuilt) clean.append(rebuilt);
    } else if (textChild(element, child)) {
      clean.append(doc.createTextNode(child.textContent));
    }
  }
  return clean;
}

export function sanitizeMermaidSvg(source, doc, prefix) {
  if (source.length > 1_000_000) throw new Error("Diagram is too large");
  const parsed = new doc.defaultView.DOMParser().parseFromString(source, "image/svg+xml");
  const root = parsed.documentElement;
  if (root.localName !== "svg" || parsed.querySelector("parsererror") || parsed.querySelectorAll("*").length > 5000) throw new Error("Invalid SVG");
  const clean = rebuild(root, doc, uniqueIds(root, prefix), semanticShapes(root));
  if (!clean?.hasAttribute("viewBox")) throw new Error("Missing diagram bounds");
  clean.setAttribute("role", "img");
  clean.setAttribute("aria-label", "Mermaid diagram");
  return clean;
}
