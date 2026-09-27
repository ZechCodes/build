// A reference becomes a link, or it stays the words the agent typed.
//
// #56. core/markdownRefs.js says what a reference IS; this says where one goes.
// The split matters: the syntax is settled and testable without a router, and
// every href here comes from core/router.js `hashFromRoute` — nothing builds a
// hash by hand, so a route that changes shape takes its links with it.
//
// # Never a broken link
//
// The resolver is injected and optional. With none — which is every caller
// today — a reference renders as the words that were typed. With one, a target
// it cannot find renders the same way. So "an agent pointed at something that
// is not there" reads as prose rather than as a link to nowhere, and that is
// true by construction rather than by remembering to check.
//
// The resolver answers where a thing is, and this file decides which surface
// that means. It is asked only for what a reference actually names:
//
//   task(number)  → { deviceId, projectId, taskId, title? }
//   workspace(name)→ { deviceId, projectId, workspaceId, name? }
//   agent(id)      → { deviceId, projectId, workspaceId?, agentId, name? }

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
 *  link means by the workspace. */
const workspacePlace = (found, extra) => ({
  name: "workspace",
  deviceId: found.deviceId,
  projectId: found.projectId,
  workspaceId: found.workspaceId,
  tab: "changes",
  ...extra,
});

/** What each kind of reference resolves to: the route it opens and the words
 *  the reader sees on hover. A kind that answers null is left as text. */
const PLACES = {
  task(reference, links) {
    const found = links.task?.(reference.number);
    if (!found?.taskId) return null;
    return {
      route: { name: "trackerTask", deviceId: found.deviceId, projectId: found.projectId, taskId: found.taskId,
        ...(reference.commentId ? { commentId: reference.commentId } : null) },
      title: found.title ? `Task #${reference.number} — ${found.title}` : `Task #${reference.number}`,
    };
  },

  workspace(reference, links) {
    const found = links.workspace?.(unesc(reference.name));
    if (!found?.workspaceId) return null;
    return { route: workspacePlace(found), title: `Workspace ${found.name || reference.name}` };
  },

  agent(reference, links) {
    const found = links.agent?.(unesc(reference.id));
    if (!found?.agentId) return null;
    // An agent standing in a workspace is reached through that workspace; the
    // project's own agent is reached on the project page.
    const route = found.workspaceId
      ? workspacePlace(found, { agent: found.agentId })
      : { name: "project", deviceId: found.deviceId, projectId: found.projectId, agent: found.agentId };
    return { route, title: found.name ? `Agent ${found.name}` : `Agent ${reference.id}` };
  },

  file(reference, links) {
    const found = links.workspace?.(unesc(reference.workspace));
    if (!found?.workspaceId) return null;
    const path = unesc(reference.path);
    return {
      route: workspacePlace(found, { tab: "files", file: path, ...(reference.line ? { line: reference.line } : null) }),
      title: reference.line ? `${path} line ${reference.line}` : path,
    };
  },
  commit(reference, links) {
    const found = links.workspace?.(unesc(reference.workspace));
    if (!found?.workspaceId) return null;
    return { route: workspacePlace(found, { commit: reference.sha }), title: `Commit ${reference.sha}` };
  },
};

/** One reference as an anchor, or null when nothing answers for it. The label
 *  is the reference exactly as written — already escaped, this running over
 *  escaped HTML — so the reader presses the words the agent typed. */
function anchorFor(reference, links) {
  const place = PLACES[reference.kind]?.(reference, links);
  if (!place) return null;
  const href = hashFromRoute(place.route);
  if (!href || href === "#/inbox") return null; // the router had nowhere to send it
  return `<a href="${esc(href)}" title="${esc(place.title)}">${reference.raw}</a>`;
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
    const anchor = anchorFor(reference, links);
    if (anchor) out = out.slice(0, reference.start) + anchor + out.slice(reference.end);
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
