// The project's agent rail, as every page standing IN a project mounts it: the
// project page, and one issue of its tracker. The rail is part of the shell a
// reader stands in, not of the page — the project's agent is reachable from
// everywhere in the project, so the bubble that opens it must not come and go
// with the page under it.
//
// The owner of the project's conversation is read from the cache first: the
// machine's project list (the sync layer keeps it on disk) names it for every
// project that has one, so opening a page asks the bridge nothing and the rail
// is up whether or not the connection is. Only a project with no owner listed
// asks the bridge, through `project.ensure_conversation` — the project's half
// of what `workspace.ensure_conversation` is for a workspace: it answers the
// owner the project already has, or mints one over a scratch directory Build
// owns. Should that call fail (a session dropped on a phone, mostly), the rail
// still comes up the moment the sync layer writes an owner into the list.
//
// Nothing here names a harness, model or effort. What a project agent starts
// on is the DEVICE's setting, held by the bridge beside its default harness,
// and the mint reads it there — so a new browser is never asked for something
// the machine that runs the agent already holds.
//
// The route may name an agent (`?agent=…`, core/router.js `conversationRoute`):
// a link to the conversation a message came from lands here, and the rail
// comes up standing on it.

import { $ } from "../dom.js";
import { mountAgentRail } from "./agentRail.js";
import { notifyError } from "./notify.js";
import { readCached, subscribeCache } from "./localCache.js";

const PROJECTS_RECORD_KIND = "projects";

/** Where this device keeps its project list, or null for a context with no
 *  cache scope (a test standing a page up without one). */
const projectsAddress = (context) => context.cacheScope?.address({ entityId: "", kind: PROJECTS_RECORD_KIND }) || null;

/** The project's conversation owner as the cached list says it: null while
 *  the list is cold, or while the project has no conversation yet. */
async function cachedOwner(context, projectId) {
  const address = projectsAddress(context);
  const listed = address ? (await readCached(address))?.value : null;
  const row = (listed || []).find((project) => project.project_id === projectId);
  return (row && (row.entity_id || row.run_id)) || null;
}

/**
 * Mount the project's rail beside the page. Resolves to the rail, to a stand-in
 * that will mount it when the cache learns the owner (the bridge could not be
 * asked and the reader has been told), or to null when the page went away
 * while the owner was being found (`disposed()`).
 */
export async function mountProjectAgentRail(options) {
  const { context, route, disposed = () => false } = options;
  const cached = await cachedOwner(context, route.projectId);
  if (disposed()) return null;
  return cached ? mountOn(options, cached) : mountOnAnswer(options);
}

/** The bridge's answer for a project the list names no owner for: the owner it
 *  has, or the one it mints. A call that fails leaves the page waiting on the
 *  list instead, and says so. */
async function mountOnAnswer(options) {
  const { context, route, disposed = () => false } = options;
  try {
    const answer = await context.rpc("project.ensure_conversation", { project_id: route.projectId });
    if (disposed()) return null;
    return mountOn(options, answer?.entity_id || answer?.run_id || null);
  } catch (error) {
    if (disposed()) return null;
    notifyError("No conversation for this project", error.message || String(error));
    return mountWhenListed(options);
  }
}

const mountOn = (options, entityId) => mountAgentRail($("#agent-rail"), railContext(options, entityId));

/** The rail, once the sync layer lists an owner for the project. Until then a
 *  handle that only knows how to stop waiting. */
function mountWhenListed(options) {
  const { context, route, disposed = () => false } = options;
  const address = projectsAddress(context);
  if (!address) return null;
  let rail = null;
  let unsubscribe = null;
  const tryMount = async () => {
    const owner = await cachedOwner(context, route.projectId);
    if (!owner || rail || disposed()) return;
    unsubscribe?.();
    unsubscribe = null;
    rail = mountOn(options, owner);
  };
  unsubscribe = subscribeCache(address, () => void tryMount());
  return {
    dispose() {
      unsubscribe?.();
      unsubscribe = null;
      rail?.dispose?.();
    },
  };
}

/** What the rail is mounted on: the project's conversation, on its owner,
 *  through this machine's caller, cache and repository. */
const railContext = ({ context, route, selection, projectName = "" }, entityId) => ({
  kind: "project",
  projectId: route.projectId,
  projectName,
  entityId,
  deviceId: context.deviceId,
  callRpc: context.rpc,
  cacheScope: context.cacheScope,
  chatRepository: context.chatRepository,
  selection,
  openAgentId: route.agent || null,
});
