// The assignee picker, as a pure model.
//
// Assigning IS dispatching. One tagged field says both who holds the issue and
// where the work runs, and five kinds cover every answer: the user, the
// project's own agent, an agent already standing on one of the project's
// workspaces, a new agent on one of those workspaces, or a new workspace with a
// new agent on it. So every option below says what it is about to DO — the
// reader should not discover afterwards that choosing a name cut a workspace.
//
// Who the project's agents are is read off the feed the whole app already has:
// the workspace list, and the agent digests the board carries for each
// workspace's conversation. No extra read, and the picker names an agent the
// same way every other surface does.
//
// No DOM, no app imports.

import { entityIdOf } from "./entityId.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { assigneeKey } from "./trackerModel.js";

/** The conversation owner a workspace's agents stand on — the same derivation
 *  the rail's own grouping makes (core/inbox.js `workspaceEntries`). */
const ownerOf = (workspace) => workspace.entity_id || workspace.run_id || workspace.id;

/** The board row holding one workspace's conversation, which is where its agent
 *  digests are. Keyed by the pair, because both machines mint a `proj-1` and a
 *  bare owner id says nothing about which one. */
function conversationsByOwner(items) {
  const rows = new Map();
  for (const item of items || []) {
    const entityId = entityIdOf(item);
    if (entityId) rows.set(JSON.stringify([item.projectKey, entityId]), item);
  }
  return rows;
}

/** What an agent is called in this project: its workspace and its place on
 *  that workspace's strip. An agent has no name of its own — it has an ordinal
 *  and a pattern — so the workspace is what makes one agent tell from another. */
const agentName = (workspaceName, agent, index) => `${workspaceName} · Agent ${agent.ordinal || index + 1}`;

/**
 * Every workspace of one project, with the agents standing in it.
 *
 * `feed` is the merge every surface reads (core/taskFeed.js) and `projectKey`
 * is the account-wide (device, project) name — never the bare `proj-N`, which
 * two machines both mint.
 */
export function workspaceAgents(feed, projectKey) {
  const rows = conversationsByOwner(feed?.items || []);
  return (feed?.workspaces || [])
    .filter((workspace) => workspace.projectKey === projectKey)
    .map((workspace) => {
      const name = workspaceDisplayName(workspace);
      const row = rows.get(JSON.stringify([projectKey, ownerOf(workspace)]));
      return {
        workspaceId: workspace.workspace_id || workspace.id,
        name,
        agents: (row?.agents || []).map((agent, index) => ({
          id: agent.id,
          label: agentName(name, agent, index),
          provider: agent.provider || "",
        })),
      };
    });
}

/** Every agent of the project by id, named. What `actorName`
 *  (core/trackerLineWords.js) is handed so an assignee, a comment's author and
 *  an event's actor all read as the same agent. */
export function agentLabels(groups) {
  const labels = {};
  for (const group of groups || []) {
    for (const agent of group.agents) labels[agent.id] = agent.label;
  }
  return labels;
}

/** What this project is called on this device, for the surfaces that name its
 *  own agent after it (`actorName`) and draw its face (core/issueAvatar.js).
 *
 *  Matched on the account-wide key and never the bare `proj-N`: two machines
 *  both mint one, and naming an agent after the wrong project is worse than
 *  naming it after none. */
export const projectName = (feed, projectKey) =>
  (feed?.projects || []).find((project) => project.projectKey === projectKey)?.name || "";

/** Every agent of the project by the harness it runs on, for the pictures the
 *  issue page draws beside comments (core/issueAvatar.js). Off the same list
 *  `agentLabels` is cut from, so an agent is drawn by the record it is named
 *  by; an agent whose record names no harness is left out rather than entered
 *  as an empty one, because "unknown harness" and "no such agent" both mean
 *  the generic mark and only one of them is worth a key. */
export function agentProviders(groups) {
  const providers = {};
  for (const group of groups || []) {
    for (const agent of group.agents) if (agent.provider) providers[agent.id] = agent.provider;
  }
  return providers;
}

/** The two options that need more asked before they can run, and the controls
 *  each one opens. Nothing else opens any. */
export const WORKSPACE_FORM = "workspace";
export const AGENT_FORM = "agent";

const standingOptions = () => [
  { id: "none", kind: "unassign", label: "Unassigned", hint: "Nobody holds it. Nothing running is stopped.", group: "" },
  { id: "user", kind: "user", label: "You", hint: "Nothing is dispatched.", group: "" },
  {
    id: "project_agent",
    kind: "project_agent",
    label: "Project agent",
    hint: "Hands it to this project's own agent and starts it.",
    group: "",
  },
];

const workspaceOptions = (group) => [
  ...group.agents.map((agent) => ({
    id: `agent:${agent.id}`,
    kind: "agent",
    agentId: agent.id,
    workspaceId: group.workspaceId,
    label: agent.label,
    hint: "Delivers the issue into this agent's conversation.",
    group: group.name,
  })),
  {
    id: `new_agent:${group.workspaceId}`,
    kind: "new_agent",
    workspaceId: group.workspaceId,
    label: `New agent in ${group.name}`,
    hint: "Starts another agent on this workspace and hands it the issue.",
    group: group.name,
    form: AGENT_FORM,
  },
];

const newWorkspaceOption = () => ({
  id: "new_workspace",
  kind: "new_workspace",
  label: "New workspace and agent",
  hint: "Cuts a workspace in this project and starts an agent on it.",
  group: "",
  form: WORKSPACE_FORM,
});

/**
 * Every answer the picker offers, in one flat list.
 *
 * Flat rather than nested: `group` is the heading a row sits under, so the same
 * list draws as a menu, as a set of optgroups, or as the quick-assign popover
 * on a row without any of them re-deriving the grouping.
 */
export function assigneeOptions(groups) {
  return [
    ...standingOptions(),
    ...(groups || []).flatMap(workspaceOptions),
    newWorkspaceOption(),
  ];
}

/** Which option an issue's current assignee is, so the picker can tick it. An
 *  assignee whose agent has left the project ticks nothing — the record still
 *  names it and the page still shows it, but it is not an answer on offer. */
export const selectedOptionId = (assignee) => assigneeKey(assignee);

/**
 * The harness, model and effort, exactly as `agentChoiceParams` answered them.
 *
 * `issues.assign` is a wire verb, and a wire verb takes `provider` — `harness`
 * is the word the MCP tools use for the same field. So the choice rides
 * through unchanged, which is also what keeps the rule that matters here:
 * ABSENT IS ABSENT. A choice nobody made is left off the object rather than
 * sent as null, because `agent.add` reads a key's presence to tell "run it on
 * this" from "run it on whatever the workspace runs on".
 */
const agentChoiceFields = (choice) => ({ ...(choice || null) });

/** How each kind builds its assignee. A table rather than a ladder: the five
 *  kinds are a closed set, and the wire shape of each is one line. */
const ASSIGNEE_BY_KIND = Object.freeze({
  unassign: () => null,
  user: () => ({ kind: "user" }),
  project_agent: () => ({ kind: "project_agent" }),
  agent: (option) => ({ kind: "agent", agent_id: option.agentId }),
  new_agent: (option, extras) => ({
    kind: "new_agent",
    workspace_id: option.workspaceId,
    ...agentChoiceFields(extras.choice),
  }),
  new_workspace: (option, extras) => ({
    kind: "new_workspace",
    ...namedField("name", String(extras.name || "").trim()),
    ...namedField("isolation", extras.isolation),
    ...agentChoiceFields(extras.choice),
  }),
});

/** A field, or nothing at all. Nothing is what "the project's own setting" and
 *  "the issue's own title" are said with: `workspace.create` reads an absent
 *  isolation as the project's, and an empty string is not absent. */
const namedField = (key, value) => (value ? { [key]: value } : null);

/**
 * The `assignee` one option stands for, as the wire takes it.
 *
 * `name` and `isolation` are the new-workspace form's two fields; `choice` is
 * what `agentChoiceParams` answered for the harness/model/effort controls. A
 * new workspace given no name takes the issue's title, and one given no
 * isolation passes none at all.
 */
export function assigneeFor(option, extras = {}) {
  const build = ASSIGNEE_BY_KIND[option?.kind];
  return build ? build(option, extras) : null;
}

/** One press, as `issues.assign` params. `note` is extra instruction delivered
 *  under the issue; it is not stored on the issue, so an empty one is left
 *  off rather than sent blank. */
export function assignParams(issueId, option, extras = {}) {
  const note = String(extras.note || "").trim();
  return {
    issue_id: issueId,
    assignee: assigneeFor(option, extras),
    ...(note ? { note } : null),
  };
}

/** What the control says it is about to do, for the confirm line under it. An
 *  option that dispatches says so; the two that do not say that too, because
 *  "nothing starts" is the fact a reader most needs before pressing. */
export const optionConsequence = (option) => option?.hint || "";

/**
 * Whether choosing this option makes the caller WAIT on real work.
 *
 * Cutting a workspace is git on a real repository, and an agent cannot exist
 * until the checkout is ready — so `issues.assign` with `new_workspace` defers
 * and its reply arrives once the cut, the agent and the delivery have all
 * happened. Seconds on a small repository, minutes on a large one. Every other
 * kind is a write and a lookup and answers in milliseconds.
 *
 * The answer's shape does not change either way, so this is a fact about how
 * the control should LOOK while it waits, and nothing else reads it.
 */
export const optionWaitsOnAWorkspace = (option) => option?.kind === "new_workspace";

/**
 * A refusal, in words the reader can act on.
 *
 * `new_workspace` goes down `workspace.create`'s own path, so that call's
 * refusals surface here — and one of them is worth saying differently. The
 * machine holding its filesystem for something else is not a mistake anybody
 * made: nothing was written, the issue is untouched, and pressing again is the
 * whole of the fix. So it says that, rather than handing over a sentence about
 * filesystem operations that the reader has to decode into "try again".
 *
 * Everything else is the bridge's own words, which are written for a reader
 * already.
 */
export function assignRefusalText(message) {
  const said = String(message || "").trim();
  if (/another filesystem operation is still running/i.test(said)) {
    return "This machine is busy with another checkout. Nothing was assigned — try again in a moment.";
  }
  return said;
}
