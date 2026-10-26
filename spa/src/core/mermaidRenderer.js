// Loaded only when a complete Mermaid fence appears. No connection/API imports:
// cached markdown can render during disconnects just as every other document.
import { sanitizeMermaidSvg } from "./mermaidSvg.js";

const CACHE_LIMIT = 32;
const SOURCE_LIMIT = 16_384;
let mermaidModule;
let queue = Promise.resolve();
let sequence = 0;
const cache = new Map();

// Reject resource-bearing syntax BEFORE Mermaid's temporary measurement DOM.
// This deliberately supports text labels only. Escapes/entities, HTML/markdown
// labels, config/frontmatter, custom CSS, clicks and image/icon metadata fall
// back to source rather than relying on Mermaid's sanitizer for transient DOM.
export function assertMermaidSource(source) {
  if (source.length > SOURCE_LIMIT) throw new Error("Diagram is too large");
  if (/^\s*---/.test(source) || /%%/.test(source)) throw new Error("Diagram configuration is unsupported");
  if (/[<&\\`\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(source.replace(/<(?=[|.-])/g, ""))) throw new Error("Use plain text diagram labels");
  if (/(?:^|[;\n])\s*(?:properties|details|links|link)\b/i.test(source) || /\b(?:icon|img|image)["']?\s*:/i.test(source)) throw new Error("Actor metadata is unsupported");
  if (/\b(?:click|classDef|style|linkStyle)\b/i.test(source) || /@\{|\burl\s*\(|(?:https?|javascript|data|file|blob|vbscript)\s*:/i.test(source)) throw new Error("Diagram resources and styling are unsupported");
}

async function draw(source, theme, doc) {
  mermaidModule ??= import("mermaid");
  const { default: mermaid } = await mermaidModule;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    theme: theme === "dark" ? "dark" : "default",
    maxTextSize: SOURCE_LIMIT,
    maxEdges: 200,
    suppressErrorRendering: true,
    secure: ["secure", "securityLevel", "startOnLoad", "htmlLabels", "maxTextSize", "maxEdges", "suppressErrorRendering"],
  });
  const container = doc.createElement("div");
  // Measurement nodes are confined to an owned host and always removed, even
  // on a parser/layout exception. Input policy above protects this transient DOM.
  container.className = "md-mermaid-measure";
  container.setAttribute("aria-hidden", "true");
  doc.body.append(container);
  try {
    const { svg } = await mermaid.render(`md-render-${++sequence}`, source, container);
    return new doc.defaultView.XMLSerializer().serializeToString(sanitizeMermaidSvg(svg, doc, "md-template"));
  } finally {
    container.remove();
  }
}

function cachedDiagram(source, theme, doc) {
  const key = `${theme}\n${source}`;
  if (cache.has(key)) {
    const held = cache.get(key);
    cache.delete(key);
    cache.set(key, held);
    return held;
  }
  // initialize/render share library globals. Serialize across every controller
  // and theme, and recover the queue after a failure without hiding that error.
  const pending = queue.then(() => draw(source, theme, doc));
  queue = pending.catch(() => {});
  cache.set(key, pending);
  if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value);
  return pending;
}

export async function renderMermaid(source, theme, doc = document) {
  assertMermaidSource(source);
  const svg = await cachedDiagram(source, theme, doc);
  // A cached drawing gets fresh local marker/clip IDs for each mounted copy.
  return sanitizeMermaidSvg(svg, doc, `md-svg-${++sequence}`);
}
