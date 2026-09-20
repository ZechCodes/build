// The shell's own parts: the conversation rail and the console region.
//
// The shell is one thing, mounted once. index.html carries its regions and they
// are never torn down; core/toolbar.js repaints the bar from the route. This
// file is the other half — what goes INSIDE #agent-rail and #console-region —
// and it is here rather than in the pages for one reason: a page that mounts
// its own rail is a page that can forget one, and one did. The tracker's issue
// page (views/trackerIssueView.js) mounted none, so opening an issue on a phone
// lost the bubble strip and left the bar holding nothing but the status dot.
//
// So a route declares what it stands on, in the table below, and the shell
// mounts it. A page reads the selection handle it must share with its own body
// (`shellSelection`) and refines the one fact only it can know
// (`refineShell`); it never mounts a shell part and it never disposes one.
// A new page gets a rail whether or not anybody remembered to give it one.
//
// Keyed, so a navigation that does not change WHICH conversation is on screen
// does not remount: a project page's Workspaces ⇄ Issues, a workspace's
// directory tabs, a legacy issue URL and the tracker's URL for the same issue
// all leave the strip exactly where it was — which is the whole of what the
// reader sees when the page swaps inside the shell.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { mountAgentRail } from "./agentRail.js";
import { mountConsole } from "./console.js";
import { createAgentSelection } from "./agentSelection.js";
import { canAnswer, routeContext } from "./deviceContexts.js";
import { deviceKey } from "./deviceKey.js";
import { notifyError } from "./notify.js";
import { readCached, subscribeCache } from "./localCache.js";

/**
 * What each route stands on: the conversation its rail is of, the console's
 * address, and the key that says whether two routes are the same standing.
 *
 * A route that cannot name its work item yet — a workspace link with no
 * workspace, a project link with no project — answers nothing, and the shell
 * stands the reader nowhere rather than on a rail addressed at undefined.
 *
 * The device and the agent a URL names are added by `shellPartsForRoute` for
 * every kind at once, so no entry here can omit them.
 */
const STANDING = {
  workspace: (route) =>
    route.workspaceId &&
    route.projectId && {
      key: `workspace:${route.workspaceId}`,
      // A workspace page paints from the records even when its machine cannot
      // answer, and so does the rail — every read it makes is the cache's. So
      // the bubbles stay beside a checkout read off disk, with the device strip
      // over the page saying whose state that is.
      paintsFromRecords: true,
      // The project's agent is a bubble above the line here, not the thing the
      // page stands on. A rail that minted one to paint that bubble would give
      // every workspace page a project agent, a scratch directory and a run
      // nobody asked for; the press that opens it mints it (core/agentRail.js).
      mintsProjectConversation: false,
      rail: {
        kind: "workspace",
        workspaceId: route.workspaceId,
        projectId: route.projectId,
        projectAgent: { projectId: route.projectId },
      },
      // The console is the WORKSPACE's, not a directory's: moving between
      // directories or refs never replaces the sessions it is holding, which is
      // why no sourceId reaches it and why the key ignores one too.
      console: { kind: "workspace", workspaceId: route.workspaceId },
    },

  // An issue of the tracker stands on the PROJECT's conversation, not on one of
  // its own. A tracker issue has none: the agents its page names are workspace
  // agents it can be assigned to, and `kind: "issue"` addresses the legacy
  // multi-stage issue record, which a tracker issue id is not. So the
  // project's agent stays beside an issue of the project exactly as it is
  // beside the project page — which is also why opening an issue from the
  // Issues tab leaves the strip alone: same standing, same key.
  trackerIssue: (route) => projectStanding(route),
  project: (route) => projectStanding(route),

  // The legacy issue page is the one that does carry a conversation of its own.
  issue: (route) =>
    route.id && {
      key: `issue:${route.id}`,
      mintsProjectConversation: false,
      rail: { kind: "issue", projectId: route.projectId || null, issueId: route.id },
      console: { kind: "issue", projectId: route.projectId || null, issueId: route.id },
    },

  branch: (route) =>
    route.projectId &&
    route.branch && {
      key: `branch:${route.projectId}:${route.branch}`,
      mintsProjectConversation: false,
      rail: { kind: "branch", projectId: route.projectId, branch: route.branch },
      console: { kind: "branch", projectId: route.projectId, branch: route.branch },
    },
};

/**
 * Standing in a project: the project's own agent, on the owner its conversation
 * has. The one standing whose owner is not known from the route alone, so the
 * shell finds it before the rail goes up (`standProjectRail` below).
 *
 * No `projectAgent`, deliberately. That field asks the rail for a second half —
 * the project's bubble above a line, with the agents of the thing you are
 * standing IN below it (core/agentRailModel.js `underTheProject`). A page
 * standing on the project's own conversation has no such second thing, so both
 * halves came out as that same conversation: the project's agent drawn twice,
 * once above the line and once below. It is the workspace page's shape, and
 * only the workspace page's.
 */
const projectStanding = (route) =>
  route.projectId && {
    key: `project:${deviceKey(route.deviceId, route.projectId)}`,
    mintsProjectConversation: true,
    rail: { kind: "project", projectId: route.projectId },
    console: null,
  };

/**
 * What the shell shows for a route — or null where the route is not a place
 * with a conversation at all.
 *
 * The inbox, a capture's decision, the holding screen a legacy link waits on
 * and the settings routes are all deliberately nowhere: there is no work item
 * to converse with, and an empty strip is the honest answer. The bar, the
 * regions and the inbox rail are still there; only the bubbles are not.
 */
export function shellPartsForRoute(route = {}) {
  const parts = STANDING[route.name]?.(route);
  if (!parts) return null;
  const deviceId = route.deviceId || null;
  return {
    key: parts.key,
    mintsProjectConversation: parts.mintsProjectConversation,
    paintsFromRecords: parts.paintsFromRecords === true,
    // The agent a conversation link names (`?agent=…`, core/router.js): the
    // rail comes up standing on it, whichever kind of page it landed on.
    rail: { ...parts.rail, deviceId, openAgentId: route.agent || null },
    console: parts.console ? { ...parts.console, deviceId } : null,
  };
}

// What is mounted, and what it was mounted for. `key` is null where the route
// stands nowhere — the selection is still minted, because a page may share one
// with its body whether or not there are bubbles beside it.
let live = null;
// Invalidates an async mint: a navigation away while `project.ensure_conversation`
// is in flight must not stand a rail up on the project the reader has left.
let generation = 0;

/** The handle the rail and the page's own body both read, so a surface's polls
 *  and its review comments name the agent whose bubble is open. Minted by the
 *  shell, because the rail is the shell's — a page that minted its own would be
 *  talking to a rail that never heard of it. */
export function shellSelection() {
  return live ? live.selection : createAgentSelection();
}

/**
 * Stand the shell on a route: mount what it says, keep what is already right.
 *
 * Called by `render()` BEFORE the page paints, so the page can read the
 * selection it has to share. Returns that selection.
 */
export function standShell(route) {
  const parts = shellPartsForRoute(route);
  const context = parts ? routeContext(route) : null;
  // A machine that cannot answer — never opened here, or gone since — has
  // nothing under this link to WRITE, so the shell stands the reader nowhere
  // rather than beside a rail whose every call can only be refused. The page
  // says so on the surface (core/deviceNotice.js). What that machine's records
  // already hold can still be READ, though, so a kind that paints from them
  // keeps its bubbles: only never having opened the machine at all takes them.
  const key = parts && standable(parts, context) ? parts.key : null;
  if (live && live.key === key && key) return live.selection;
  teardown();
  live = { key, selection: createAgentSelection(), late: {}, rail: null, console: null };
  if (!key) return live.selection;
  mountShellParts(parts, context);
  return live.selection;
}

const standable = (parts, context) =>
  Boolean(context) && (canAnswer(context) || parts.paintsFromRecords);

function mountShellParts(parts, context) {
  const mine = ++generation;
  if (parts.console) {
    live.console = mountConsole($("#console-region"), {
      ...parts.console,
      call: context.rpc,
      cacheScope: context.cacheScope,
    });
  }
  if (parts.mintsProjectConversation) void standProjectRail(parts, context, mine);
  else live.rail = mountRail(parts.rail, context);
}

const PROJECTS_RECORD_KIND = "projects";

/** Where this device keeps its project list, or null for a context with no
 *  cache scope (a test standing a page up without one). */
const projectsAddress = (context) => context.cacheScope?.address({ entityId: "", kind: PROJECTS_RECORD_KIND }) || null;

/** The project as this machine's cached list has it, or null while the list is
 *  cold. Both facts the rail wants come off the one row: the conversation's
 *  owner, and the name whose initial the project's bubbles wear. */
async function cachedProject(context, projectId) {
  const address = projectsAddress(context);
  const listed = address ? (await readCached(address))?.value : null;
  return (listed || []).find((project) => project.project_id === projectId) || null;
}

/** The owner the cached row names: null while the list is cold, or while the
 *  project has no conversation yet. */
const ownerOf = (row) => (row && (row.entity_id || row.run_id)) || null;

/**
 * Stand the project's rail up on the owner of its conversation.
 *
 * The owner is read from the CACHE first: the machine's project list (the sync
 * layer keeps it on disk) names it for every project that has one, so opening a
 * page asks the bridge nothing and the rail is up whether or not the connection
 * is. Only a project with no owner listed asks, through
 * `project.ensure_conversation` — the project's half of what
 * `workspace.ensure_conversation` is for a workspace: it answers the owner the
 * project already has, or mints one over a scratch directory Build owns.
 *
 * Nothing here names a harness, model or effort. What a project agent starts on
 * is the DEVICE's setting, held by the bridge beside its default harness, so a
 * new browser is never asked for something the machine that runs the agent
 * already holds.
 */
async function standProjectRail(parts, context, mine) {
  const row = await cachedProject(context, parts.rail.projectId);
  if (generation !== mine) return; // the reader left while the owner was found
  // The name rides along whether or not the row names an owner: a project the
  // bridge is about to mint a conversation for is still a project with a name,
  // and its bubbles should wear its initial the moment they appear.
  const named = { ...parts, rail: { ...parts.rail, projectName: row?.name || "" } };
  if (ownerOf(row)) live.rail = mountRail({ ...named.rail, entityId: ownerOf(row) }, context);
  else await standOnAnswer(named, context, mine);
}

/** The bridge's answer for a project the list names no owner for. A call that
 *  fails (a session dropped on a phone, mostly) leaves the page waiting on the
 *  list instead, and says so. */
async function standOnAnswer(parts, context, mine) {
  try {
    const answer = await context.rpc("project.ensure_conversation", { project_id: parts.rail.projectId });
    if (generation !== mine) return;
    live.rail = mountRail({ ...parts.rail, entityId: answer?.entity_id || answer?.run_id || null }, context);
  } catch (error) {
    if (generation !== mine) return;
    notifyError("No conversation for this project", error.message || String(error));
    live.rail = standWhenListed(parts, context, mine);
  }
}

/** The rail, once the sync layer lists an owner for the project. Until then a
 *  handle that only knows how to stop waiting. */
function standWhenListed(parts, context, mine) {
  const address = projectsAddress(context);
  if (!address) return null;
  let rail = null;
  let unsubscribe = null;
  const tryMount = async () => {
    const row = await cachedProject(context, parts.rail.projectId);
    const owner = ownerOf(row);
    if (!owner || rail || generation !== mine) return;
    unsubscribe?.();
    unsubscribe = null;
    rail = mountRail({ ...parts.rail, projectName: row.name || "", entityId: owner }, context);
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

function mountRail(descriptor, context) {
  return mountAgentRail($("#agent-rail"), {
    ...descriptor,
    // The device's caller, not the session's: the machine can drop and resume
    // under a mounted shell, and the rail goes on asking the machine rather
    // than the socket it was built over.
    call: context.rpc,
    cacheScope: context.cacheScope,
    chatRepository: context.chatRepository,
    selection: live.selection,
    // One-shot, set right before navigating to a branch just cut from the
    // toolbar's create form. Read here because the rail is mounted here now,
    // and cleared in the same breath so the next mount does not steal focus.
    autofocusComposer: takeComposerFocus(),
    ...adoptingSupplier(descriptor),
  });
}

/** Whether this mount is the one that was asked to open with the composer
 *  focused — true at most once per navigation. */
function takeComposerFocus() {
  const wanted = App.focusComposerOnMount === true;
  App.focusComposerOnMount = false;
  return wanted;
}

/**
 * The one fact the shell cannot know: who is claiming the checkout under a
 * branch.
 *
 * Two surfaces on a branch page can mutate an unclaimed checkout first — the
 * rail's first message and the review's first comment — and near-simultaneous
 * adoptions would ask for two owners of one checkout. The page holds the single
 * adopter both take theirs from (core/adoption.js `createAdopters`), so it hands
 * it over here rather than the shell building a second one.
 *
 * Only a branch gets the indirection: `context.adopting` is a truthy test in the
 * rail, and a supplier installed on a kind whose page never fills it would
 * suppress the rail's own fallback adopter (core/agentRail.js `adoptingCall`).
 */
function adoptingSupplier(descriptor) {
  if (descriptor.kind !== "branch") return {};
  const held = live.late;
  return { adopting: () => held.adopting?.() || null };
}

/** A fact only the standing page knows, handed to the shell part that needs it.
 *  Read lazily by the rail, so this may land after the mount — which it does:
 *  the page paints after the shell stands. */
export function refineShell(facts) {
  if (live) Object.assign(live.late, facts);
}

/**
 * Take the shell's parts down and leave the regions empty.
 *
 * For the page that knows the route is standing on something that is not there
 * — a branch whose folder is not a repository yet has no conversation and no
 * checkout to open terminals in — and for the gate, which takes the whole frame.
 */
export function dropShell() {
  teardown();
  live = { key: null, selection: createAgentSelection(), late: {}, rail: null, console: null };
}

function teardown() {
  generation += 1;
  live?.rail?.dispose();
  live?.console?.dispose();
  live = null;
}

/** Teardown for tests and for a gate that tears the session down. */
export function stopShell() {
  teardown();
}
