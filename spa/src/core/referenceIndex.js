// The one index every written reference is resolved against (#229).
//
// Every markdown surface renders through core/markdown.js `markdownHtml`, and
// that asks here: so a reference means the same thing in a chat message, on a
// task page, in a plan doc and in a one-line preview. What the index holds is
// handed to it by core/referenceIndexFeed.js, out of the feed and the tracker's
// task lists — caches, never the wire — and it answers synchronously, because a
// paint cannot wait.
//
// Surfaces that paint once and keep what they painted subscribe here: a list
// landing after the paint turns prose into links, and nothing else on the
// surface would have moved.
//
// No DOM, no app imports: a test hands it sources directly.

import { referenceTables, resolverFor } from "./referenceTargets.js";

let tables = referenceTables();
let signature = "";
let version = 0;
const listeners = new Set();

/** What an answer can depend on, as one string: two sources that write the
 *  same one resolve every reference the same way. */
function signatureOf(held) {
  return JSON.stringify([
    held.held,
    held.workspaces.map((row) => [row.projectKey, row.workspaceId, row.name, row.directories]),
    held.projects.map((row) => [row.projectKey, row.name]),
    [...held.agents].map(([id, agent]) => [id, agent.workspace?.workspaceId, agent.name]),
    Object.entries(held.tasks).map(([key, list]) => [key, (list || []).map((task) => [task.id, task.number, task.title])]),
  ]);
}

/** Replace what the index answers from: `{ feed, tasks }`, as
 *  core/referenceTargets.js `referenceTables` reads them. Listeners hear only
 *  a change that could move an answer. */
export function holdReferenceSources(sources) {
  const next = referenceTables(sources);
  const nextSignature = signatureOf(next);
  tables = next;
  if (nextSignature === signature) return;
  signature = nextSignature;
  version += 1;
  listeners.forEach((listener) => listener(version));
}

/** A number that moves whenever an answer could have: what a surface folds
 *  into its paint fingerprint. */
export const referenceIndexVersion = () => version;

/** Hear every change. Returns the unsubscribe. */
export function subscribeReferenceIndex(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The resolver core/markdownLinks.js asks, for a reader at `place`. */
export const referenceResolver = (options = {}) => resolverFor(tables, options);
