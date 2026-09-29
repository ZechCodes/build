// A reference becomes a link, or it stays the words the agent typed.
//
// #56. core/markdownRefs.js says what a reference IS; this says where one goes.
// The split matters: the syntax is settled and testable without a router, and
// every href here comes from core/router.js `hashFromRoute` — nothing builds a
// hash by hand, so a route that changes shape takes its links with it.
//
// # Never a broken link
//
// A reference nothing answers for is never a link to nowhere. The resolver
// (core/referenceTargets.js `resolverFor`, asked through core/referenceIndex.js)
// answers one of three things for each:
//
//   a place     → an anchor, labelled with what it names: `#42 Rebuild the
//                 shell`, a workspace's name, an agent's, `path:10`, a
//                 workspace and a short SHA. The words the agent typed are on
//                 the hover (#229).
//   null        → looked for, and not there: the words the agent typed, marked
//                 `md-ref-missing` with the reason on the hover, so a broken
//                 reference is not mistaken for prose (#229).
//   undefined   → nothing here can say: the words as typed, untouched.
//
// What the resolver is asked, and what a place carries:
//
//   task(number)   → { deviceId, projectId, taskId, title? }
//   workspace(name)→ { deviceId, projectId, workspaceId, name?, sourceId?, directories? }
//   agent(id)      → { deviceId, projectId, workspaceId?, agentId, name? }
//   project(name)  → { deviceId, projectId, name? }

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { referencesIn } from "./markdownRefs.js";

/** Text that has been through `esc`, back as it was written. The references are
 *  read off escaped HTML, so a path carrying `&` arrives as `&amp;` and the
 *  resolver must be asked about the real one. */
const unesc = (value) =>
  value
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");

/** Where a workspace's own page is: its Changes, which is what a workspace
 *  link means by the workspace. A reference naming one source directory of it
 *  carries that directory. */
const workspacePlace = (found, extra) => ({
  name: "workspace",
  deviceId: found.deviceId,
  projectId: found.projectId,
  workspaceId: found.workspaceId,
  ...(found.sourceId ? { sourceId: found.sourceId } : null),
  tab: "changes",
  ...extra,
});

/** A task's title as a label carries it: long enough to recognise, never a
 *  paragraph in the middle of a sentence. */
const TITLE_CHARS = 60;
const clipped = (title) => (title.length > TITLE_CHARS ? `${title.slice(0, TITLE_CHARS - 1).trimEnd()}…` : title);

/** Code-shaped words in a label — a path, a SHA — set as code. */
const codeLabel = (words) => `<span class="md-ref-code">${esc(words)}</span>`;

/** How many characters of a SHA a label shows: git's own short form. */
const SHORT_SHA = 8;

/** A path in a workspace with several source directories, where the reference
 *  named none: the first segment is the directory when it names one, because
 *  the path on disk from the workspace's root starts with it. With one
 *  directory, the path is that directory's own and is taken as written. */
function directoryOfPath(found, path) {
  const directories = found.directories || [];
  if (found.sourceId || directories.length < 2) return { found, path };
  const seam = path.indexOf("/");
  const head = seam > 0 ? path.slice(0, seam) : "";
  const directory = directories.find((one) => one.name === head);
  return directory ? { found: { ...found, sourceId: directory.sourceId }, path: path.slice(seam + 1) } : { found, path };
}

const noWorkspace = (reference) => `Not found: no workspace called “${unesc(reference.workspace)}” is on record here.`;

/**
 * Each kind of reference: what to ask the resolver (`find`), where an answer
 * goes and what the reader sees (`link`), and why a missing one is missing
 * (`missing`). `find` answers a place, `null` for "looked, and it is not
 * there", or `undefined` for "nothing here can say".
 */
const KINDS = {
  task: {
    find: (reference, links) => links.task?.(reference.number),
    link: (found, reference) => ({
      route: { name: "trackerTask", deviceId: found.deviceId, projectId: found.projectId, taskId: found.taskId,
        ...(reference.commentId ? { commentId: reference.commentId } : null) },
      label: esc(`#${reference.number}${found.title ? ` ${clipped(found.title)}` : ""}${reference.commentId ? " · comment" : ""}`),
      description: `${reference.commentId ? "A comment on task" : "Task"} #${reference.number}${found.title ? ` — ${found.title}` : ""}`,
    }),
    missing: (reference) => `Not found: no task #${reference.number} is on record in this project.`,
  },

  workspace: {
    find: (reference, links) => links.workspace?.(unesc(reference.name)),
    link: (found, reference) => ({
      route: workspacePlace(found),
      label: esc(found.name || unesc(reference.name)),
      description: `Workspace ${found.name || unesc(reference.name)}`,
    }),
    missing: (reference) => `Not found: no workspace called “${unesc(reference.name)}” is on record here.`,
  },

  agent: {
    find: (reference, links) => links.agent?.(unesc(reference.id)),
    // An agent standing in a workspace is reached through that workspace; the
    // project's own agent is reached on the project page.
    link: (found, reference) => ({
      route: found.workspaceId
        ? workspacePlace(found, { agent: found.agentId })
        : { name: "project", deviceId: found.deviceId, projectId: found.projectId, agent: found.agentId },
      label: esc(found.name || unesc(reference.id)),
      description: `Agent ${found.name || unesc(reference.id)}`,
    }),
    missing: (reference) => `Not found: no agent ${unesc(reference.id)} is on record here; it may have been removed.`,
  },

  project: {
    find: (reference, links) => links.project?.(unesc(reference.name)),
    link: (found, reference) => ({
      route: { name: "project", deviceId: found.deviceId, projectId: found.projectId },
      label: esc(found.name || unesc(reference.name)),
      description: `Project ${found.name || unesc(reference.name)}`,
    }),
    missing: (reference) => `Not found: no project called “${unesc(reference.name)}” is on record here.`,
  },

  file: {
    find: (reference, links) => links.workspace?.(unesc(reference.workspace)),
    link(found, reference) {
      const place = directoryOfPath(found, unesc(reference.path));
      const at = reference.line ? `:${reference.line}` : "";
      return {
        route: workspacePlace(place.found, { tab: "files", file: place.path, ...(reference.line ? { line: reference.line } : null) }),
        label: codeLabel(`${place.path}${at}`),
        description: `${place.path}${reference.line ? ` line ${reference.line}` : ""} in ${found.name || unesc(reference.workspace)}`,
      };
    },
    missing: noWorkspace,
  },

  commit: {
    find: (reference, links) => links.workspace?.(unesc(reference.workspace)),
    link: (found, reference) => ({
      route: workspacePlace(found, { commit: reference.sha }),
      label: `${esc(found.name || unesc(reference.workspace))} · ${codeLabel(reference.sha.slice(0, SHORT_SHA))}`,
      description: `Commit ${reference.sha} in ${found.name || unesc(reference.workspace)}`,
    }),
    missing: noWorkspace,
  },
};

/** A reference that was looked for and is not there: the words the agent
 *  typed, marked so the reader can tell it from prose, with why on the hover. */
const missingHtml = (reference, why) =>
  `<span class="md-ref-missing" title="${esc(why)}">${reference.raw}</span>`;

/** One reference as markup, or null to leave the words exactly as typed. A
 *  resolved one is an anchor whose label says what it names — the words the
 *  agent typed stay on the hover (#229). */
function markupFor(reference, links) {
  const kind = KINDS[reference.kind];
  const found = kind?.find(reference, links);
  if (found === undefined) return null;
  if (!found) return missingHtml(reference, kind.missing(reference));
  const place = kind.link(found, reference);
  const href = hashFromRoute(place.route);
  if (!href || href === "#/inbox") return null; // the router had nowhere to send it
  const title = `${place.description} — ${unesc(reference.raw)}`;
  return `<a class="md-ref" href="${esc(href)}" title="${esc(title)}">${place.label}</a>`;
}

/// Code and existing anchors are already complete HTML. URL text and Markdown
/// link targets may carry `#42` as a fragment, where it is part of that URL.
/// Keep each whole span literal before looking for Build references beside it.
const PROTECTED_SPAN = /<code>[\s\S]*?<\/code>|<a\b[^>]*>[\s\S]*?<\/a>|\]\([^\s)]*\)|(?:https?:\/\/|www\.)[^\s<>"']+/g;

/** The stretches of `html` that are outside protected spans, in order. */
function outsideProtected(html) {
  const spans = [];
  let at = 0;
  PROTECTED_SPAN.lastIndex = 0;
  for (let match = PROTECTED_SPAN.exec(html); match; match = PROTECTED_SPAN.exec(html)) {
    spans.push({ text: html.slice(at, match.index), open: true });
    spans.push({ text: match[0], open: false });
    at = match.index + match[0].length;
  }
  spans.push({ text: html.slice(at), open: true });
  return spans;
}

/** Every reference in one stretch replaced by its anchor, right to left so the
 *  positions behind each splice still hold. */
function expandRun(text, links) {
  let out = text;
  for (const reference of referencesIn(text).reverse()) {
    const markup = markupFor(reference, links);
    if (markup) out = out.slice(0, reference.start) + markup + out.slice(reference.end);
  }
  return out;
}

/**
 * Expand every reference in already-escaped HTML.
 *
 * `links` is optional; without it nothing is expanded and the text is returned
 * as it came, which is what keeps every existing caller of the renderer exactly
 * as it was.
 */
export function expandReferences(html, links = null) {
  if (!links || !html) return html;
  return outsideProtected(html)
    .map((span) => (span.open ? expandRun(span.text, links) : span.text))
    .join("");
}

/** The words a resolved reference's link would carry, as plain text; null when
 *  it does not resolve, so the words stay as typed. */
function wordsFor(reference, links) {
  const kind = KINDS[reference.kind];
  const found = kind?.find(reference, links);
  return found ? unesc(kind.link(found, reference).label.replace(/<[^>]*>/g, "")) : null;
}

/// A code span in plain markdown, kept as written: its references are examples.
const CODE_SPAN = /`[^`\n]*`/g;

/** Every resolved reference in one stretch of plain text replaced by its words,
 *  right to left so the positions behind each splice still hold. */
function labelRun(text, links) {
  let out = text;
  for (const reference of referencesIn(text).reverse()) {
    const words = wordsFor(reference, links);
    if (words) out = out.slice(0, reference.start) + words + out.slice(reference.end);
  }
  return out;
}

/**
 * Plain text with each reference that resolves read as the words its link
 * would carry (#229) — for a one-line preview, where there is no link to press
 * and `[[ws:commit:…]]` would be machinery. Code spans stay as written.
 */
export function plainReferences(text, links = null) {
  const source = text || "";
  if (!links || !source) return source;
  let out = "";
  let at = 0;
  CODE_SPAN.lastIndex = 0;
  for (let match = CODE_SPAN.exec(source); match; match = CODE_SPAN.exec(source)) {
    out += labelRun(source.slice(at, match.index), links) + match[0];
    at = match.index + match[0].length;
  }
  return out + labelRun(source.slice(at), links);
}
