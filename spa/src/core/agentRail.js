// The agent rail: the conversation surface for whatever the view area is
// standing in.
//
// It is two things. The BUBBLE STRIP is pinned to the right edge below the
// toolbar and never goes away: one bubble per agent, carrying that agent's
// unread count and whether it is working, so the state of every agent on this
// work item is legible with the rail fully collapsed. The strip is also the
// agent selector — there is no menu. Tapping a bubble expands the CONVERSATION
// PANEL to its left; tapping another switches which conversation is in it.
//
// The panel is the whole of talking to an agent. Chat is the conversation and
// the box you write in; TUI swaps the same panel onto that agent's PTY, sized
// to the panel. There are no Conversation and Agent tabs any more: this is both
// of them, beside the work instead of instead of it.
//
// One work item, one rail: `mountAgentRail` is given the branch or the issue,
// polls the bridge for it (branch.get / issue.get), and every agent it renders
// comes off that payload's agents[].
//
// With one exception, and it is the project's agent. A project's agent is
// reachable from every workspace in the project — that is what makes it the
// project's — so a workspace's rail carries it as one bubble above a line, and
// pressing it stands this same rail on the project's conversation without the
// page leaving the workspace. The rail is on one of the two at a time and reads
// the other beside it, so the strip says what both are doing.

import { App, go } from "../app.js";
import { createPatternRenderer } from "./agentCanvas.js";
import { hashString } from "./patternMotion.js";
import { watchChanges } from "./changeEvents.js";
import { createAdoptingCall } from "./adoption.js";
import { agentDefaultsForWorkspace, agentDefaultsInWorkspace } from "./workspaceDefaults.js";
import { projectAgentChoiceOf } from "./projectAgentSetting.js";
import { workspaceKey } from "./deviceKey.js";
import {
  AGENT_STARTING,
  QUIET_SHAPE,
  agentCanInterrupt,
  agentHasTerminal,
  agentIsUp,
  agentSessionAnswered,
  agentStartFailure,
  agentHeading,
  agentRemovalWho,
  agentWho,
  canRemoveAgent,
  providerLabel,
  railBubbles,
  railEntity,
  railStatusShape,
  railWorkStatus,
  removeAgentConfirm,
  selectAgentId,
  startFailuresLearned,
} from "./agentRailModel.js";
import { railStatusLeadClass, railStatusLeadHtml, railWhoHtml } from "./agentRailRender.js";
import { createGitStatusTicker } from "./gitStatusTicker.js";
import { createAgentSelection } from "./agentSelection.js";
import { NO_AGENT_CHOICE, activeModelLabel, chosenProviderId, reconcileAgentChoice } from "./agentChoice.js";
import { confirmActionAt } from "./confirm.js";
import {
  insertRecord,
  isProvisionalKey,
  patchRecord,
  projectPending,
  removeRecord,
} from "./optimistic.js";
import { EXITING_ATTRIBUTE, patchList, rekeyEntry } from "./patchList.js";
import { hide, motionSettled, reveal, setMotionRowHtml } from "./motion.js";
import { composerHtml, mountComposerModelMenu } from "./composer.js";
import { catalogForProvider, creatableCatalog, effortLevels, effortSupported, matchCatalogModel, modelParams } from "./modelPicker.js";
import { markSeen } from "./inboxView.js";
import { notifyError } from "./notify.js";
import { deviceFeedView } from "./deviceContexts.js";
import { deviceCatalog } from "./inboxDevices.js";
import { createConversationCache } from "./conversationCache.js";
import { createChatRepository } from "./chatRepository.js";
import { createAgentRailContext } from "./agentRailContext.js";
import { fileLinkRoute } from "./threadLinks.js";
import { entityIdOf } from "./entityId.js";
import { replyOrNothing } from "./session.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  MUTATION_THREAD_PAGE,
  SMALLEST_THREAD_PAGE,
  activityRunKeyAt,
  activityRunThroughAt,
  chatPaintFingerprint,
  createThreadCache,
  digestsOf,
  paintThreadEntries,
  paintThreadKeepingPlace,
  readThroughSequence,
  pressedActivityRunKey,
  revealThreadSequence,
  threadOfferState,
  timelineEntries,
  wireThreadAttachments,
  wireThreadOptions,
  wireThreadComposer,
  threadItemKey,
  wireThreadLinks,
  wireThreadRevisionLinks,
} from "./thread.js";
import { createActivityRuns } from "./activityRuns.js";
import { isAtBottom } from "./paintKeepingPlace.js";
import { unreadAnchorSequence } from "./unreadAnchor.js";
import { wireExpansionReveal } from "./revealExpanded.js";
import { runDigestToFetch } from "./activityDigest.js";
import { timedPaint } from "./paintTiming.js";
import { mountAgentSurfaces, openSurfaceOverlay } from "./agentSurfaces.js";
import { mountAgentObservation } from "./agentObservation.js";
import { createTaskCompletionTracker } from "./taskCompletionModel.js";
import { mountTaskCompletionToast } from "./taskCompletionToast.js";
import { surfaceMenuOptions, surfacesAfterGrace } from "./agentSurfacesModel.js";
import { surfaceSessionGeneration } from "./surfacesCache.js";
import { menuButtonMarkup, mountMenuIfChanged } from "./splitButton.js";
import { mountAgentTab } from "./surfaceTabs.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { PIN_CLASS, pinButtonHtml, syncPinButton } from "./pinControl.js";
import { providerInSameFamily } from "./providerCatalog.js";
import { mountComposerClearance } from "./composerClearance.js";
import { createChatPanelMotion } from "./chatPanelMotion.js";
import { createChatTitleMotion } from "./chatTitleMotion.js";
import "../styles/shell.css";

/** How often the rail re-reads its work item. The same cadence the detail
 *  surfaces have always polled at: fast enough that a reply appears while you
 *  are still looking at the panel. */
const RAIL_POLL_MS = 1600;
/** How close to the top of the conversation counts as asking for the page
 *  above it. Not zero: a reader flicking upwards should have the history on
 *  its way before they land, so the join is one they scroll through rather
 *  than wait at. */
const OLDER_ITEMS_TRIGGER_PX = 120;

/** Where the reader's pin choice is kept. Named for what the flag used to mean
 *  — the panel out or shut — and holding what it means now: docked beside the
 *  work, or a popover on the bubble strip. One key, so a reader who had the
 *  panel out keeps a pinned panel. */
const PINNED_KEY = "build.rail.expanded";
/** The thing this rail's pin docks, as the reader would name it. */
const PANEL_SUBJECT = "conversation";
const POPOVER_CLASS = "rail-popover";
const COLLAPSED_CLASS = "rail-collapsed";
const ANCHOR_PROPERTY = "--rail-anchor";
const COMPOSER_IDS = { input: "railinput", send: "railsend", hint: "railhint" };
const RAIL_STATUS_ID = "rail-status";
const RAIL_STATUS_LEAD_ID = "rail-status-lead";
const RAIL_STATUS_PILLS_ID = "rail-status-pills";
const RAIL_STATUS_GIT_ID = "rail-status-git";
const RAIL_VIEWER_ID = "rail-surfaces-viewer";
const RAIL_OBSERVATION_ID = "rail-observation";
const WORKING_WORD_SELECTOR = ".rail-status-working-word";
const STATUS_TEXT_SELECTOR = ".rail-status-text";
const STANDING_PILL_SELECTOR = `.surface-pill:not([${EXITING_ATTRIBUTE}])`;
const SURFACE_MENU_CLASS = "rail-surface-menu";
const SURFACE_MENU_SELECTOR = `.${SURFACE_MENU_CLASS}`;
const SURFACE_MENU_LABEL = "⋮";
const SURFACE_MENU_TITLE = "Open a surface";
const AGENT_NOT_YET_BORN = "ghost";

// What makes this page's faces this page's own. An agent's pattern is drawn
// from its id, so without a salt every agent would move exactly the same way on
// every load, forever. One salt per page load, shared by every rail on it: the
// same agent looks like itself all session and like something else tomorrow.
const PATTERN_SEED_SALT = Math.floor(Math.random() * 0x100000000);

/** The seed the painter moves an agent's pattern by. */
const patternSeed = (agentId) => (hashString(agentId) ^ PATTERN_SEED_SALT) >>> 0;

/** Amber for a dark theme, the value the sheet's `--amber` carries. Read only
 *  where there is no cascade to read the token off — a canvas needs a colour it
 *  can actually paint in, and `var(--amber)` is not one. */
const UNREAD_INK = "#ffd447";

const operationIsUncertain = (error) =>
  error?.uncertain === true || (error?.timedOut === true && error?.uncertain !== false);

const providerChoiceIsCompatible = (offered, chosen, requested) =>
  !!requested && (requested === chosen || providerInSameFamily(offered.providers, requested) === chosen);

const clampKnownModelChoice = (provider, providerCatalog, choice, matchedModel) => {
  const efforts = effortLevels(providerCatalog.efforts || [], matchedModel);
  const effort = effortSupported(providerCatalog.models || [], matchedModel.id) && efforts.includes(choice.effort)
    ? choice.effort
    : "";
  return { provider, model: matchedModel.id, effort };
};

const clampStoredAgentChoice = (catalog, choice) => {
  const offered = creatableCatalog(catalog || {});
  const provider = chosenProviderId(offered, choice);
  if (!providerChoiceIsCompatible(offered, provider, choice.provider)) return { provider, model: "", effort: "" };
  const providerCatalog = catalogForProvider(offered, provider);
  const matchedModel = matchCatalogModel(providerCatalog.models || [], choice.model || "");
  if (matchedModel) return clampKnownModelChoice(provider, providerCatalog, choice, matchedModel);
  return { provider, model: choice.model || "", effort: choice.effort || "" };
};

const recoveryExcerpt = (recovery) => {
  const body = recovery.body.trim();
  if (body) return body.length > 80 ? `${body.slice(0, 77)}…` : body;
  return `${recovery.attachmentCount} attachment${recovery.attachmentCount === 1 ? "" : "s"}`;
};

const recoveryHeading = (recovery) => {
  if (recovery.kind === "creation") return "Agent creation uncertain";
  if (recovery.status === "execution_error") return "Message saved; agent did not start";
  return recovery.status === "uncertain" ? "Delivery uncertain" : "Message not sent";
};

const recoveryAction = (recovery) => {
  if (!recovery.canRetry) return "";
  const label = recovery.kind === "creation" ? "Retry creation" : recovery.status === "uncertain" ? "Check delivery" : "Retry";
  return `<button type="button">${label}</button>`;
};

const chatRecoveryHtml = (controller) => controller.recoveries().map((recovery) =>
  `<div class="chat-recovery-entry" data-operation="${esc(recovery.operationId)}">
    <span><strong>${recoveryHeading(recovery)}</strong>
      <span>${esc(recoveryExcerpt(recovery))}</span>
      ${recovery.error ? `<span>${esc(recovery.error)}</span>` : ""}
    </span>
    ${recoveryAction(recovery)}
  </div>`,
).join("");

const conversationIdOf = (agent) => agent?.conversation_id || agent?.id || "";

/** What the rail talks through: the cache and the conversations of the machine
 *  the surface above it is standing on, both handed down by that surface. A
 *  standalone mount (the pane suites) brings only a caller, and the rail makes
 *  its own repository over it. */
function railChatDependencies(context) {
  const cacheScope = context.cacheScope;
  const injectedRepository = context.chatRepository;
  return {
    cacheScope,
    ownsRepository: !injectedRepository,
    repository: injectedRepository || createChatRepository({
      scope: cacheScope || {},
      viewingContext: context.viewingContext || App.viewingContext,
      call: (method, params) => context.call(method, params),
    }),
  };
}

function createRailChatOwnership(repository, key, entityOf) {
  let provisional = null;
  const addressFor = (entity, agent) => {
    const execution = entity.executionContext;
    if (execution) {
      return {
        entityId: execution.entity_id,
        agentId: execution.agent_id,
        conversationId: execution.conversation_id,
      };
    }
    return { entityId: entity.entityId, agentId: agent.id, conversationId: conversationIdOf(agent) };
  };
  return {
    controllerFor(agent) {
      const entity = entityOf();
      if (!agent || !entity.entityId) return null;
      const address = addressFor(entity, agent);
      const controller = repository.controller(address);
      const digest = entity.executionContext?.agent || agent;
      controller.absorbAgent({ ...digest, id: address.agentId });
      return controller;
    },
    provisional() {
      const entity = entityOf();
      if (!provisional) {
        provisional = repository.provisional(`new:${key}`, {
          entityId: entity.entityId || key,
          conversationId: `new:${key}`,
        });
      }
      return provisional;
    },
    resolve(controller, identity) {
      return repository.resolveProvisional(controller, identity);
    },
    releaseProvisional() {
      provisional = null;
    },
  };
}

const fallbackCacheEntityId = (context, feedRow) => {
  if (context.kind === "issue") return context.issueId;
  return feedRow ? entityIdOf(feedRow) : null;
};
const cacheEntityId = (identity, context, feedRow) => identity?.entityId || fallbackCacheEntityId(context, feedRow);
const cacheAgentId = (identity, selectedId) => identity?.agentId || selectedId || "";
const cacheConversationId = (identity, selectedId) => identity?.conversationId || selectedId || "";

function addressedCacheIdentity({ cacheScope, context, feedRow, selectedId, controller }) {
  if (!cacheScope) return null;
  const identity = controller?.identity;
  const entityId = cacheEntityId(identity, context, feedRow);
  if (!entityId) return null;
  const agentId = cacheAgentId(identity, selectedId);
  const conversationId = cacheConversationId(identity, selectedId);
  return cacheScope.address({ entityId, agentId, conversationId });
}

const computedStyleOf = (element) =>
  element && typeof globalThis.getComputedStyle === "function" ? globalThis.getComputedStyle(element) : null;

/** The colour a bubble's pattern paints in while nothing is waiting on it: the
 *  bubble's own ink, so the palette stays stated in the sheet — dim for a ghost,
 *  accent for the conversation that is open. */
const restingInk = (button) => {
  const computed = computedStyleOf(button);
  return (computed && computed.color) || "";
};

/** The colour of something waiting to be read, the same token the count above
 *  it is drawn in. */
const unreadInk = () => {
  const computed = computedStyleOf(document.documentElement);
  const token = computed ? String(computed.getPropertyValue("--amber") || "").trim() : "";
  return token || UNREAD_INK;
};

/** Which painter belongs to which bubble. An agent's is its id; a ghost has no
 *  id and there is only ever one of it. */
const faceKey = (type, agentId) => (type === "agent" ? `agent:${agentId || ""}` : type);

const bubbleKey = (bubble) => faceKey(bubble.type, bubble.id);

// What survives a remount. The rail is rebuilt whenever the view under it is
// (a tab switch re-renders the surface), so the human's choices — which agent
// is open, whether the panel is out, chat or TUI, and anything typed but not
// sent — are kept here rather than in the DOM that is about to be replaced.
// What the next agent on a work item will be created as — the harness card the
// human pressed and the model menu's selection, held until there is an agent to
// write them to. Nothing goes to the bridge until then: there is no entity
// choice worth writing for a checkout that may never be adopted.
// Chat or TUI, per work item: the terminal is the basement, so walking into a
// different branch or issue starts you in the conversation whatever face of the
// last one you were looking at.

/** Forget what the rail remembers. For tests, and for a session teardown — the
 *  drafts and choices belong to the person who was signed in. */
export function resetAgentRailMemory() {
  // Chat/view memory now belongs to the injected application repository and
  // is retired with its account/device scope. Kept for old test harnesses.
}

/** The width the panel stops sitting beside the work and is laid over it
 *  instead (styles/shell.css, `@media (max-width: 760px)`). */
const PANEL_OVERLAYS_BELOW = 761;

/** Whether the panel is docked before anyone has said. Beside the work it is:
 *  the conversation and the work are both on screen and neither costs the other
 *  anything. Laid over the work it is not, or a workspace opens showing its
 *  conversation and nothing else — no Files, no Changes, and nothing on screen
 *  saying the strip is the way back to them. A reader who has made the choice
 *  keeps it, at either width. */
const readPinned = () => {
  try {
    const remembered = localStorage.getItem(PINNED_KEY);
    if (remembered !== null) return remembered !== "0";
  } catch {
    /* private mode: the default below is the whole answer */
  }
  return window.innerWidth >= PANEL_OVERLAYS_BELOW;
};

const writePinned = (on) => {
  try {
    localStorage.setItem(PINNED_KEY, on ? "1" : "0");
  } catch {
    /* private mode: the choice lasts the session */
  }
};

/** Which way the popover faces, which is which way the strip runs: down the
 *  view's right edge on a desktop, across its foot on a phone. */
const stripRunsAcross = () => window.innerWidth < PANEL_OVERLAYS_BELOW;

/** The count over a bubble's face, hidden while nothing is waiting. The `+` is
 *  a control rather than a conversation, so nothing is ever waiting on it. */
const bubbleCountHtml = (bubble) => {
  if (bubble.type === "add") return "";
  if (!bubble.unread) return `<span class="rail-count" hidden></span>`;
  return `<span class="rail-count">${esc(String(bubble.unread))}</span>`;
};

/** A pattern IS the bubble's face, so it takes the label's place: a canvas for
 *  core/agentCanvas.js to paint into. The `+`, and the project's own initial,
 *  speak in a glyph and keep a label. */
const bubbleFaceHtml = (bubble) => {
  const count = bubbleCountHtml(bubble);
  if (bubble.pattern) return `<canvas class="rail-glyph" aria-hidden="true"></canvas>${count}`;
  return `<span class="rail-bubble-label">${esc(bubble.label)}</span>${count}`;
};

const bubbleClasses = (bubble) => {
  const classes = ["rail-bubble", `rail-bubble-${bubble.type}`];
  if (bubble.active) classes.push("active");
  if (bubble.working) classes.push("working");
  if (bubble.starting) classes.push("starting");
  return classes.join(" ");
};

export function bubbleHtml(bubble) {
  // The line between the project's agent and this work item's own. It says
  // nothing and is pressed by nobody, so it is not a button.
  if (bubble.type === "separator") return `<div class="rail-sep" role="separator"></div>`;
  const pattern = bubble.pattern ? ` data-pattern="${esc(String(bubble.pattern))}"` : "";
  return `<button type="button" class="${bubbleClasses(bubble)}" data-bubble="${esc(bubble.type)}"
    data-agent="${esc(bubble.id)}"${pattern} title="${esc(bubble.title)}"
    aria-label="${esc(bubble.title)}">${bubbleFaceHtml(bubble)}</button>`;
}

export function syncStripPainters(bubbles, painted, faces) {
  bubbles.forEach((bubble, index) => {
    const face = faces.get(bubbleKey(bubble));
    if (!face) return;
    face.renderer.setWorking(!!bubble.working);
    // An unread count sits centred on the face, so the face gets out of its
    // way: dimmed, and in the same amber the number is drawn in.
    const unread = bubble.unread > 0;
    const ink = unread ? unreadInk() : restingInk(painted[index]);
    if (face.ink !== ink) {
      face.ink = ink;
      face.renderer.setInk(ink);
    }
    if (face.dimmed !== unread) {
      face.dimmed = unread;
      face.renderer.setDimmed(unread);
    }
  });
}

function paintStatusLead(lead, status) {
  const shape = railStatusShape(status);
  if (lead.dataset.shape !== shape) {
    lead.dataset.shape = shape;
    lead.className = railStatusLeadClass(shape);
    lead.innerHTML = railStatusLeadHtml(shape);
  }
  if (shape === QUIET_SHAPE) hide(lead, { axis: "width" });
  else reveal(lead, { axis: "width" });
  const text = lead.querySelector(STATUS_TEXT_SELECTOR);
  if (text) text.textContent = status.working || status.starting;
}

/// One ticker per git line, kept with the element it paints so a rail that is
/// rebuilt around it does not lose the characters standing there.
const gitTickers = new WeakMap();

function gitTickerFor(git) {
  const standing = gitTickers.get(git);
  if (standing) return standing;
  const ticker = createGitStatusTicker(git);
  gitTickers.set(git, ticker);
  return ticker;
}

/// The git facts move a character at a time rather than being redrawn: digits
/// roll in place, a section that arrives cascades in, one that goes cascades
/// out. core/gitStatusTicker.js does the moving.
function paintStatusGit(git, status) {
  return gitTickerFor(git).show(status.git);
}

const railStatusRowHtml = () =>
  `<div class="rail-status" id="${RAIL_STATUS_ID}" hidden>
    <span class="rail-status-lead" id="${RAIL_STATUS_LEAD_ID}" hidden></span>
    <div class="rail-status-pills scrollstrip" id="${RAIL_STATUS_PILLS_ID}" role="group" aria-label="Agent surfaces"></div>
    <span class="rail-status-git mono" id="${RAIL_STATUS_GIT_ID}" hidden></span>
  </div>`;

const railViewerHostHtml = () => `<div class="rail-surfaces-viewer" id="${RAIL_VIEWER_ID}" hidden></div>`;

const railObservationHostHtml = () =>
  `<div class="agent-observation-host" id="${RAIL_OBSERVATION_ID}" hidden></div>`;

function surfaceMenuHtml(options) {
  return options.length ? menuButtonMarkup(SURFACE_MENU_LABEL, options, { title: SURFACE_MENU_TITLE, icon: true }) : "";
}

function surfaceMenuRegionHtml(options) {
  return `<span class="${SURFACE_MENU_CLASS}">${surfaceMenuHtml(options)}</span>`;
}

/** What the remove button says it will do, in the same words as the
 *  confirmation it opens (core/agentRailModel.js's `agentRemovalWho`). */
const removeButtonTitle = (removalWho) => `Remove ${removalWho} from this branch`;

/** The button that takes this agent off the branch, or nothing when it cannot be. */
function railRemoveButtonHtml(removalWho, removable) {
  if (!removable) return "";
  const removeTitle = removeButtonTitle(removalWho);
  return `<button type="button" class="btn mini rail-remove" title="${esc(removeTitle)}"
        aria-label="${esc(removeTitle)}">Done</button>`;
}

/** The TUI toggle, or nothing for an agent with no basement to show. */
function railTuiButtonHtml(mode, hasTerminal) {
  if (!hasTerminal) return "";
  const showingTui = mode === "tui";
  const tuiTitle = showingTui ? "Back to the conversation" : "Show the terminal";
  return `<button type="button" class="rail-mode rail-tui${showingTui ? " on" : ""}"
        aria-pressed="${showingTui}" title="${tuiTitle}">TUI</button>`;
}

export function panelHeadHtml(who, mode, { provider = "", removable = false, hasTerminal = true, surfaceOptions = [], heading = null, pinned = true, removalWho = "" } = {}) {
  return `<div class="rail-head">
    ${harnessIconHtml(provider)}
    ${railWhoHtml(who, heading)}
    ${railTuiButtonHtml(mode, hasTerminal)}
    ${railRemoveButtonHtml(removalWho || who, removable)}
    ${pinButtonHtml({ subject: PANEL_SUBJECT, pinned })}
    ${surfaceMenuRegionHtml(surfaceOptions)}
  </div>`;
}

/** The project's conversation as a context of its own: this device's caller,
 *  cache and conversations, pointed at the project above the work item. Its
 *  agent is not the work item's selected agent, so the shared selection handle
 *  is deliberately left behind. */
const projectAgentContext = (context, known) => ({
  ...context,
  kind: "project",
  projectId: context.projectAgent.projectId,
  entityId: known.entityId,
  workspaceId: null,
  selection: null,
  openAgentId: null,
  projectAgent: { ...context.projectAgent, ...known },
});

/** The work item's own context again, with the agent whose bubble asked for it
 *  open. */
const workItemContext = (context, known, openAgentId) => ({
  ...context,
  openAgentId,
  projectAgent: { ...context.projectAgent, ...known },
});

/**
 * Mount the rail for one work item.
 *
 * `context` is `{ kind: "branch", projectId, branch }` or
 * `{ kind: "issue", projectId, issueId }`, optionally carrying a `selection`
 * (core/agentSelection.js) — the shared handle the surface beside the rail
 * reads, so its polls and its review comments name the agent whose bubble is
 * open — and an `adopting` supplier, the view's adopter for the checkout under
 * it (core/adoption.js `createAdopters`), so the rail does not claim a checkout
 * a sibling surface is claiming too. Returns `{ dispose() }`; disposing tears
 * down the client view only — PTYs and conversations are the daemon's.
 *
 * `projectAgent: { projectId }` asks for a second conversation on the same
 * rail: the project's agent, one bubble above a line, reachable from every
 * workspace in the project because that is what makes it the PROJECT's. The
 * rail stands on one of the two at a time and keeps the other beside it, and a
 * press moves it between them by standing this same host on the other context.
 * Everything worth keeping across that — which agent was open, what was typed,
 * the history already read — lives in the injected repository keyed by the
 * context, so it is all still there on the way back.
 */
export function mountAgentRail(host, context) {
  if (!host) return { dispose() {} };
  if (!context.projectAgent?.projectId) return mountRailOnContext(host, context, null);
  // What the rail has learned about the project: its name, and the conversation
  // owner — minted by the press that opens it, never by a render. Held out here
  // so a swap does not ask for either of them again.
  let known = { entityId: context.projectAgent.entityId || null, name: context.projectAgent.name || "" };
  let live = null;
  const stand = (standing, alongside) => {
    live?.dispose();
    live = mountRailOnContext(host, { ...standing, alongside }, swap);
  };
  const swap = {
    learned: (facts) => {
      known = { ...known, ...facts };
    },
    toProject: (entityId) => {
      known = { ...known, entityId };
      stand(projectAgentContext(context, known), workItemContext(context, known, null));
    },
    toWorkItem: (openAgentId) => {
      stand(workItemContext(context, known, openAgentId), projectAgentContext(context, known));
    },
  };
  swap.toWorkItem(null);
  return {
    dispose() {
      live?.dispose();
      live = null;
    },
  };
}

/** What a rail knows about the project above it before it has read anything:
 *  whether it was asked for one at all, whether it is the project's own
 *  conversation it is standing on, and the name and owner the swap before this
 *  mount already learned. */
function projectAgentState(context) {
  const projectAgent = context.projectAgent || null;
  const standing = !!projectAgent && context.kind === "project";
  return {
    projectAgent,
    standing,
    entityId: (standing ? context.entityId : projectAgent?.entityId) || null,
    name: projectAgent?.name || "",
  };
}

/** Which conversation the rail was asked to keep beside the one it stands on. */
const alongsideKind = (context) => context.alongside?.kind || "project";

/** Whose conversation this mount opens on: the bubble that asked to come back
 *  here, or the one this rail was last left on. */
const openingAgentId = (context, railView) => context.openAgentId || railView.selectedAgentId();

/** The rail standing on ONE of its contexts. `swap` is how it moves to the
 *  other, and is null for a rail that has only one. */
function mountRailOnContext(host, context, swap) {
  const completionTracker = createTaskCompletionTracker();
  const completionToast = mountTaskCompletionToast(host);
  const railContext = createAgentRailContext(context);
  const key = railContext.key;
  const { cacheScope, ownsRepository: ownsChatRepository, repository: chatRepository } = railChatDependencies(context);
  const {
    isPending,
    projectOptimistic,
    provisionalKey,
    reconcileOptimistic,
    runOptimistic,
    subscribeOptimistic,
  } = chatRepository.optimisticStore();
  const railView = chatRepository.railView(key);
  const selection = context.selection || createAgentSelection();
  let entity = railEntity(null, context.kind);
  let selectedId = openingAgentId(context, railView);
  selection.set(selectedId);
  // ---- the project's agent, where the view asked for one --------------------
  // The rail stands on one conversation and keeps the other beside it: the
  // project's when this is the workspace, the workspace's when this is the
  // project. `alongside` is the one it is NOT standing on, read for the bubbles
  // of it the strip carries and for nothing else.
  const { projectAgent, standing: onProjectAgentRail, entityId: knownOwner, name: knownName } = projectAgentState(context);
  let projectOwner = knownOwner;
  let projectName = knownName;
  let mintingProjectAgent = false;
  let alongside = context.alongside && createAgentRailContext(context.alongside);
  let alongsideEntity = railEntity(null, alongsideKind(context));
  // Docked beside the work, or a card on the strip. The pin is the reader's
  // remembered layout choice; visibility belongs to this visit and never
  // rewrites that choice.
  let pinned = readPinned();
  // An unpinned rail has no composer to focus at all — the human just cut this
  // branch and is about to type into it, so that intent outranks whatever they
  // left the rail at on the last one.
  let panelVisible = pinned || context.autofocusComposer === true;
  const panelOut = () => panelVisible;
  let mode = railView.panelMode();
  let poll = null;
  let disposed = false;
  let tui = null; // the mounted PTY pane, in TUI mode
  let titleMotion = null;
  const transientThreadCache = createThreadCache();
  let threadCache = transientThreadCache;
  // Choosing the next agent's harness, with the chooser in the panel. Entered
  // by the strip's `+`, left by the send that creates the agent or by opening
  // any existing bubble.
  let addingAgent = false;
  let threadAgentId = null; // whose conversation the cache holds
  let loadingOlderItems = false; // a page of history is in flight
  let olderItemsAwaitingPaint = false;
  let activityRuns = null; // the open runs of the conversation in the panel
  let activityRunsFor = null; // whose conversation those runs belong to
  let unreadFrom = null; // where the unread line stands in that conversation
  let reportedRead = 0; // how far this panel has told the daemon it read
  let reportedFloor = null; // and how much of the conversation it held saying so
  let paintedChat = null; // what the timeline in the panel was drawn from
  let paintedDigests = []; // the run totals that timeline was drawn with
  let seededSurfaces = null;
  const chatOwnership = createRailChatOwnership(chatRepository, key, () => entity);
  // An optimistic agent is the provisional controller gaining a visible card,
  // not a second conversation. Keep its draft, pending sends, and eventual
  // resolved identity on the controller that created it.
  const controllerForAgent = (agent) => isProvisionalKey(agent?.id) ? null : chatOwnership.controllerFor(agent);
  const provisionalController = () => chatOwnership.provisional();

  const cacheIdentity = () => {
    if (disposed) return null;
    const addressed = addressedCacheIdentity({
      cacheScope,
      context,
      feedRow,
      selectedId,
      controller: controllerForAgent(agentOf(selectedId)),
    });
    if (!addressed) return null;
    return {
      ...addressed,
      surfaceSessionGeneration: surfaceSessionGeneration(agentOf(selectedId)?.surface_session_generation),
    };
  };

  let conversationCache = null;

  const createBoundConversationCache = () => createConversationCache({
    addressOf: cacheIdentity,
    threadCache,
    onThreadSeeded: (seededFor) => {
      if (disposed) return;
      threadAgentId = seededFor;
      paintChat();
    },
    onSurfacesSeeded: (seen) => {
      if (disposed) return;
      seededSurfaces = {
        surfaces: surfacesAfterGrace(seen.surfaces, seen.at, Date.now()),
        generation: seen.generation,
        at: seen.at,
      };
      syncSurfaces();
      paintSurfaceMenu();
    },
  });

  const bindConversationCache = () => {
    const controller = controllerForAgent(agentOf(selectedId));
    const wantedThreadCache = controller?.history.threadCache || transientThreadCache;
    if (conversationCache && threadCache === wantedThreadCache) return conversationCache;
    threadCache = wantedThreadCache;
    conversationCache = createBoundConversationCache();
    absorbedThreadPayload = null;
    seededSurfaces = null;
    return conversationCache;
  };

  const absorbSurfaces = () => {
    seededSurfaces = null;
    const agent = agentInFocus();
    bindConversationCache().absorbSurfaces(agent?.surfaces ?? null, agent?.surface_session_generation);
  };

  const resetConversationCache = () => {
    conversationCache = null;
    absorbedThreadPayload = null;
    seededSurfaces = null;
    olderItemsAwaitingPaint = false;
  };
  // The one payload already folded through the cache. A repaint hands the same
  // payload back, and absorbing it twice is not idempotent for the one shape
  // that matters: an uncursored (full) answer REPLACES the window, so a seed
  // that landed between two paints of the same stale payload would be thrown
  // away — which is a conversation blanking under its reader.
  let absorbedThreadPayload = null;
  // Which agent the payload in hand was READ FOR. Not the same question as
  // threadAgentId: that one is about the cache, this one is about the answer the
  // cache would be filled from. Between opening another agent's bubble and its
  // read landing, the payload still belongs to the agent just left, and its
  // words must not be drawn under the new one's name.
  let threadOwner = null;
  let adopting = null;
  let catalog = null; // models.list, once it lands: the harnesses and their models
  // settings.get's `project_agent`, on a project's rail: what this machine says
  // a new project agent starts on. Null off a project's rail and until the
  // machine answers.
  let projectAgentSetting = null;
  let creating = null;
  let ensuringConversation = null;
  const faces = new Map();
  let agentlessOnce = false; // an answer that lost the agents, waiting to be repeated
  let feedRow = null; // this work item's row off the shared feed, for the pinned status line
  let statusTicker = null;
  // One-shot: the composer steals focus the first time it paints, then never
  // again — a poll rebuilding the panel later (a new agent, a mode switch)
  // must not keep yanking focus back while the human is doing something else.
  let autofocusComposerPending = context.autofocusComposer === true;
  // The pinned box's controllers: the send, for the one thing on it a poll can
  // move — which shape it is wearing — and the model menu on the other side of
  // the row. Null whenever the panel is not showing the conversation.
  let composerControl = null;
  let disposeComposerClearance = null;
  let composerModelMenu = null;
  let composerController = null;
  let unsubscribeComposerController = null;
  let surfacesBlock = null;
  let observationBlock = null;
  let surfaceOverlay = null; // the surface a menu option opened, over the panel
  let closeSurfaceMenu = null; // shuts the head's ⋯, and with it its outside-press watch
  let panelMotion = null;


  const agentIdOf = (agent) => agent.id;

  const openConversation = (agentId) => {
    chooseAgent(agentId);
    resetConversationCache();
    // Let threadWindow bind the selected controller's canonical history before
    // reading. Marking the new id here would make it keep the old cache.
    threadAgentId = null;
  };
  const pendingAgentsScope = () => `${chatRepository.scopeKey}:agents:${key}`;
  const pendingThreadScope = (agentId) => `${chatRepository.scopeKey}:thread:${key}:${agentId || ""}`;
  const visibleAgents = () => projectOptimistic(pendingAgentsScope(), entity.agents, { keyOf: agentIdOf });

  const agentOf = (id) => visibleAgents().find((agent) => agent.id === id) || null;
  /** Open this agent's conversation, and tell everything else on screen: the
   *  bubble strip is the selector for the whole work item, not just the rail. */
  const chooseAgent = (id) => {
    selectedId = id || null;
    railView.chooseAgent(selectedId);
    if (!isProvisionalKey(selectedId)) {
      const addressed = controllerForAgent(agentOf(selectedId));
      selection.set(addressed?.identity.agentId || selectedId);
    }
  };
  const controllerInFocus = () => controllerForAgent(agentInFocus()) || provisionalController();
  const conversationKey = (controller = controllerInFocus()) => {
    return `${controller.identity.entityId || key}:${controller.identity.agentId || controller.identity.draftId || AGENT_NOT_YET_BORN}`;
  };

  // ---- the agent that does not exist yet -------------------------------------

  /** The two agents this view offers, with the account's answer folded in: one
   *  of them IS the account's default harness (`models.list`'s
   *  `default_provider`), so the carrier question is never asked here. */
  const creatable = () => creatableCatalog(catalog || {});

  /** What the first send on this work item will create: whatever the human has
   *  said here, over the account's default harness until a card is pressed —
   *  and clamped onto what is actually offered, so a choice held from before
   *  the account moved its default highlights a card rather than none.
   *  Resolved once, so the cards, the composer's menu and the `agent.add`
   *  params cannot disagree. */
  const newAgentChoice = () => {
    const held = provisionalController().choice();
    const said = held.provider
      ? { provider: held.provider, model: held.requestedModel, effort: held.effort }
      : NO_AGENT_CHOICE;
    return { ...said, provider: chosenProviderId(creatable(), said) };
  };
  const writeNewAgentChoice = (next) => provisionalController().setProvisionalChoice(next);

  /** The account-wide name of the workspace this rail stands in, or null
   *  outside one: a workspace's own agent defaults (core/workspaceDefaults.js)
   *  layer over the account's, and only a rail inside one has any to layer. */
  const railWorkspaceKey = () =>
    railContext.kind === "workspace" ? workspaceKey(railContext.deviceId, railContext.workspaceId) : null;

  /** Which preference a new agent on this rail leads with: on a project's rail
   *  the DEVICE's project-agent setting (core/projectAgentSetting.js), inside a
   *  workspace that workspace's own slot, the account's anywhere else. A
   *  project agent talks ABOUT a project rather than working in a checkout, so
   *  what it starts on is the machine's answer and not this browser's. */
  const onProjectRail = () => railContext.kind === "project";
  const deviceProjectAgent = () => projectAgentSetting || NO_AGENT_CHOICE;
  const newAgentDefaults = (catalogOffered) =>
    onProjectRail()
      ? deviceProjectAgent()
      : agentDefaultsInWorkspace(railWorkspaceKey(), catalogOffered);
  /** The same, for a harness the human has just pressed. A model belongs to its
   *  harness, so the device's model travels only onto the harness the device
   *  named; any other leads with that harness's own default. */
  const newAgentDefaultsFor = (providerId) =>
    onProjectRail()
      ? { ...(deviceProjectAgent().provider === providerId ? deviceProjectAgent() : NO_AGENT_CHOICE), provider: providerId }
      : agentDefaultsForWorkspace(railWorkspaceKey(), providerId);

  const seedNewAgentDefaults = () => {
    if (!catalog || provisionalController().choice().provider) return;
    writeNewAgentChoice(clampStoredAgentChoice(catalog, newAgentDefaults(catalog)));
  };

  /** That choice as `agent.add` params: empties omitted, so the harness's own
   *  default stands where nothing was said. */
  const newAgentParams = (choice = newAgentChoice()) => {
    const { models } = catalogForProvider(catalog || {}, choice.provider);
    return modelParams(models || [], choice.model || choice.requestedModel, choice.effort, choice.provider);
  };

  /** The adopting caller for a checkout Build owns nothing in — an external
   *  worktree, which is the only kind there is to adopt.
   *
   *  A view whose other surfaces can adopt too owns the adopter and hands it
   *  down (`context.adopting`), so the rail and the Changes review claim the
   *  checkout once between them. Standing alone, the rail makes its own once
   *  the payload says which checkout it is, and keeps it — it holds the run it
   *  mints. */
  const adoptingCall = (call) => {
    if (context.adopting) return context.adopting() || null;
    if (!adopting && entity.adoptable && entity.projectId) {
      adopting = createAdoptingCall(call, entity.projectId, entity.worktreeId);
    }
    return adopting;
  };

  // ---- the pinned status line ------------------------------------------------

  /// The route the shared feed's rows are keyed by — the same identity
  /// core/toolbarModel.js's `toolbarIdentity` reads for the toolbar's own
  /// jump menu, matched here to find this work item's row.
  const feedRoute = () => railContext.feedRoute();

  const loadedConversationItems = () => {
    const window = threadCache.readWindow();
    return (window && window.items) || [];
  };

  const statusRow = () => host.querySelector(`#${RAIL_STATUS_ID}`);

  const showRowIfPopulated = () => {
    const row = statusRow();
    if (!row) return;
    const lead = row.querySelector(`#${RAIL_STATUS_LEAD_ID}`);
    const git = row.querySelector(`#${RAIL_STATUS_GIT_ID}`);
    const pills = row.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const populated = !lead.hidden || gitTickerFor(git).populated() || !!pills.querySelector(STANDING_PILL_SELECTOR);
    if (populated) reveal(row, { axis: "height" });
    else hide(row, { axis: "height" });
  };

  const collapseWorkingUnderPills = () => {
    const row = statusRow();
    if (!row) return;
    const word = row.querySelector(WORKING_WORD_SELECTOR);
    if (!word) return;
    if (row.querySelector(`#${RAIL_STATUS_PILLS_ID}`).querySelector(STANDING_PILL_SELECTOR)) {
      hide(word, { axis: "width" });
    } else {
      reveal(word, { axis: "width" });
    }
  };

  const syncRailStatusRow = () => {
    showRowIfPopulated();
    collapseWorkingUnderPills();
  };

  const paintRailStatus = () => {
    if (!panelVisible) return;
    const row = statusRow();
    if (!row) return;
    const openAgentLabel = providerLabel((agentOf(selectedId) || {}).provider);
    const status = railWorkStatus(feedRow, Date.now(), loadedConversationItems(), openAgentLabel, agentInFocus());
    paintStatusLead(row.querySelector(`#${RAIL_STATUS_LEAD_ID}`), status);
    paintStatusGit(row.querySelector(`#${RAIL_STATUS_GIT_ID}`), status).then(syncRailStatusRow);
    syncRailStatusRow();
    motionSettled().then(syncRailStatusRow);
  };

  const unsubscribePending = subscribeOptimistic(pendingAgentsScope(), () => paint());

  const unsubscribeFeed = subscribeFeed((feed) => {
    // One machine's rows, not the merge: this work item is on the machine its
    // link named, and every machine mints a `proj-1` — so the row is looked for
    // by the machine and the project together.
    feedRow = toolbarIdentity(feedRoute(), deviceFeedView(feed, context.deviceId)).row;
    paintRailStatus();
  });

  // ---- reading the work item ------------------------------------------------

  /// One read of the work item: its agents, and the conversation of the one
  /// whose bubble is open.
  ///
  /// `agent_id` names that conversation. A daemon that does not yet read it
  /// answers with the entity's own — the first agent's — which is what every
  /// surface before the rail asked for; the param is here so the panel follows
  /// the bubble as soon as the daemon can tell them apart.
  const detail = async () => {
    const call = chatRepository.currentCall();
    await bindConversationCache().seed();
    const addressed = controllerForAgent(agentOf(selectedId));
    const agentId = addressed?.identity.agentId || selectedId;
    const askedAgentId = agentId && !isProvisionalKey(agentId) ? { agent_id: agentId } : {};
    const scope = { ...threadCache.cursorParam(), ...askedAgentId };
    return railContext.detail(call, scope);
  };

  const letGoOfRefusedAgent = (error, asked) => {
    if (!asked || isProvisionalKey(asked)) return;
    if (!/agent_id/.test((error && error.message) || "")) return;
    openConversation(null);
  };

  const answerLostTheAgents = (answered) => {
    if (answered.agents.length || !visibleAgents().length || agentlessOnce) return false;
    agentlessOnce = true;
    return true;
  };

  const readIsStale = (asked) => disposed || asked !== selectedId;

  const showTaskCompletions = (agents) => {
    completionTracker.forgetMissing(agents.map((agent) => agent.id));
    for (const agent of agents) {
      const completions = completionTracker.observe(agent);
      if (completions.length) completionToast.show(agent.id, completions.map((task) => task.title));
    }
  };

  const refresh = async () => {
    refreshAlongside();
    const asked = selectedId;
    let payload;
    try {
      payload = await detail();
    } catch (error) {
      if (readIsStale(asked)) return;
      letGoOfRefusedAgent(error, asked);
      // Anything else — a branch that stopped resolving (finished, renamed) —
      // leaves the rail as it was rather than blanking the conversation under
      // the reader.
      return;
    }
    if (readIsStale(asked)) return;
    // The human opened a different bubble while this read was in flight: it
    // answers about the conversation they just left, and folding its delta into
    // the cache the switch just cleared would show one agent's words under
    // another's name. Drop it; the next tick asks about the right one.
    const answered = railEntity(payload, context.kind);
    if (answerLostTheAgents(answered)) return;
    // A start that never reached a harness is answered here and nowhere else:
    // the daemon replied to the press long before the spawn, so this push is
    // the first word about it. Said once, where the throw used to land.
    for (const failed of startFailuresLearned(entity.agents, answered.agents)) {
      notifyError("Could not start the agent", agentStartFailure(failed));
    }
    agentlessOnce = false;
    entity = answered;
    for (const agent of answered.agents) controllerForAgent(agent);
    reconcileOptimistic(pendingAgentsScope(), answered.agents, { keyOf: agentIdOf });
    chooseAgent(selectAgentId(visibleAgents(), selectedId));
    controllerForAgent(agentOf(selectedId))?.reconcileUncertain().then(syncChatRecovery);
    // Whose conversation this payload carries: the agent we asked about, or —
    // when we asked about none, which is every first read — the entity's own,
    // which is the agent the selection just landed on (its first).
    if (!isProvisionalKey(selectedId)) threadOwner = asked === null ? selectedId : asked;
    absorbSurfaces();
    paint();
    showTaskCompletions(answered.agents);
  };

  // ---- the conversation beside this one -------------------------------------

  /// What the rail has just learned about the project. Told to the mount that
  /// holds this rail's two contexts, so a swap in either direction starts from
  /// it rather than asking again.
  const learnProjectFacts = ({ entityId, name }) => {
    projectName = name || projectName;
    if (entityId && projectOwner !== entityId) {
      projectOwner = entityId;
      if (context.alongside?.kind === "project") {
        alongside = createAgentRailContext({ ...context.alongside, entityId });
      }
    }
    swap?.learned({ entityId: projectOwner, name: projectName });
  };

  /// The project's own row off `project.list`, or nothing when that machine has
  /// no such project.
  const listedProjectRow = (listed) =>
    (listed?.projects || []).find((project) => project.project_id === projectAgent.projectId);

  /// What the project above this work item is called, and whether it has a
  /// conversation yet. `project.list` is the only read that answers the second
  /// without minting one (planning/v2/workspaces.md), and it answers the first
  /// in the same breath. Asked once, at mount: the press is what mints, and a
  /// bubble waiting to be started is a true thing to show until then.
  const readProjectAgent = async () => {
    if (!projectAgent || (projectOwner && projectName)) return;
    const call = chatRepository.currentCall();
    const row = listedProjectRow(await call("project.list", {}).catch(() => null));
    if (disposed || !row) return;
    learnProjectFacts({ entityId: row.entity_id || row.run_id, name: row.name });
    paint();
    refreshAlongside();
  };

  /// What this machine says a new project agent starts on, asked once and only
  /// where it is spent: the rail standing on a project's conversation. A rail
  /// on a work item never mints a project agent, so it never asks.
  ///
  /// A refusal reads as no preference, which is what a machine that will not
  /// answer offers — the catalog's own default harness then stands, exactly as
  /// it does for a device that has chosen nothing.
  const readProjectAgentSetting = async () => {
    if (!onProjectRail()) return null;
    const call = chatRepository.currentCall();
    const settings = await call("settings.get", {}).catch(() => null);
    return settings && projectAgentChoiceOf(settings);
  };

  /// One read of the conversation this rail is not standing on — the project's
  /// from a workspace, the workspace's from inside the project's — for what its
  /// bubbles say. There is nothing to read while the project has no owner: that
  /// bubble says how to start one instead, which is not a read.
  const refreshAlongside = async () => {
    if (!alongside || (alongside.kind === "project" && !projectOwner)) return;
    const call = chatRepository.currentCall();
    // A refusal leaves the strip saying what it said: a bubble that blanks
    // because one read missed is worse than one a tick behind.
    const payload = await alongside.detail(call, SMALLEST_THREAD_PAGE).catch(() => null);
    if (disposed || !payload) return;
    alongsideEntity = railEntity(payload, alongside.kind);
    paint();
  };

  /// The project's bubble, or null on a rail nobody asked for one on. Its
  /// agents are whichever side of the swap the project is on.
  const projectAgentEntry = () =>
    projectAgent && {
      name: projectName || projectAgent.projectId,
      entityId: projectOwner,
      agents: onProjectAgentRail ? visibleAgents() : alongsideEntity.agents,
      active: onProjectAgentRail,
    };

  // ---- painting -------------------------------------------------------------

  const releaseFaces = () => {
    faces.forEach((face) => face.renderer.destroy());
    faces.clear();
  };

  const wireBubble = (button, bubble) => {
    // The line is not a control: nothing to press, nothing to drop on.
    if (bubble.type === "separator") return;
    button.onclick = () => pressBubble(button.dataset.bubble, button.dataset.agent);
    wireBubbleDrop(button);
    const canvas = bubble.pattern ? button.querySelector("canvas.rail-glyph") : null;
    if (!canvas) return;
    faces.set(bubbleKey(bubble), {
      renderer: createPatternRenderer({
        canvas,
        patternIndex: bubble.pattern,
        seed: patternSeed(bubble.id || ""),
      }),
      ink: null,
      dimmed: false,
    });
  };

  /// Whether a file dropped on this bubble has a composer to land in. The `+`
  /// is a chooser, and a bubble standing for a conversation this rail is not on
  /// — the project's, or a workspace agent's from inside the project's — is a
  /// doorway rather than a place to leave a file.
  const bubbleTakesFiles = (type) => type === "ghost" || (type === "agent" && !onProjectAgentRail);

  /// A file dragged onto a bubble is for that agent: the drop opens its
  /// conversation and lands the file in the composer, as if it had been dropped
  /// on the box. Only a bubble that IS the open conversation's takes one — a
  /// file is not an answer to "which harness", nor to "which conversation".
  const wireBubbleDrop = (button) => {
    if (!bubbleTakesFiles(button.dataset.bubble)) return;
    const dragged = (event) => [...(event.dataTransfer?.types || [])].includes("Files");
    const over = (event) => {
      if (!dragged(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      button.classList.add("is-dropping");
    };
    button.addEventListener("dragenter", over);
    button.addEventListener("dragover", over);
    button.addEventListener("dragleave", () => button.classList.remove("is-dropping"));
    button.addEventListener("drop", (event) => {
      button.classList.remove("is-dropping");
      const files = [...(event.dataTransfer?.files || [])];
      if (!files.length) return;
      event.preventDefault();
      event.stopPropagation();
      dropFilesOnBubble(button.dataset.bubble, button.dataset.agent, files);
    });
  };

  /// Files waiting for a composer: a drop that switched conversations lands
  /// before the new conversation's box is wired when its history is still
  /// loading, so they are held and handed over when it is.
  let droppedFiles = null;
  const deliverDroppedFiles = () => {
    if (!droppedFiles || !composerControl) return;
    const files = droppedFiles;
    droppedFiles = null;
    composerControl.addFiles(files);
  };
  const dropFilesOnBubble = (type, agentId, files) => {
    droppedFiles = files;
    if (type === "agent" && agentId && (agentId !== selectedId || addingAgent)) {
      openAgent(agentId);
      paint();
      refresh();
    }
    if (!panelOut()) showPanel();
    deliverDroppedFiles();
  };

  const releaseFacesLeftBehind = (keysPainted) => {
    for (const [name, face] of [...faces]) {
      if (keysPainted.has(name)) continue;
      face.renderer.destroy();
      faces.delete(name);
    }
  };

  const paintStrip = (strip, bubbles) => {
    const painted = patchList(strip, bubbles, { keyOf: bubbleKey, render: bubbleHtml, wire: wireBubble });
    releaseFacesLeftBehind(new Set(bubbles.map(bubbleKey)));
    syncStripPainters(bubbles, painted, faces);
  };

  const paint = () => {
    if (disposed) return;
    if (!host.querySelector(".rail-strip")) {
      releaseFaces();
      host.innerHTML = `<div class="rail-strip"></div>`;
    }
    const strip = host.querySelector(".rail-strip");
    paintStrip(strip, railBubbles({
      // Below the line are the work item's agents, whichever side of the swap
      // this rail is standing on.
      agents: onProjectAgentRail ? alongsideEntity.agents : visibleAgents(),
      selectedId, kind: entity.kind, chatCapable: entity.chatCapable !== false,
      addingAgent,
      projectAgent: projectAgentEntry(),
    }));
    let panel = host.querySelector("#rail-panel");
    if (panelOut() && !panel) {
      panel = document.createElement("div");
      panel.className = "rail-panel";
      panel.id = "rail-panel";
      host.insertBefore(panel, strip);
    }
    if (panelOut()) paintPanel();
    syncPopover();
  };

  // ---- docked, or a card on the strip ---------------------------------------

  /** Which bubble the popover points at, and where its notch sits along the
   *  panel's edge to point there — down the panel on a desktop, across its foot
   *  on a phone, because that is which way the strip runs. */
  const anchorPopover = (panel, showing) => {
    const bubble = showing ? host.querySelector(".rail-bubble.active") : null;
    panel.dataset.anchor = bubble?.dataset.agent || "";
    if (!bubble) return panel.style.removeProperty(ANCHOR_PROPERTY);
    const box = bubble.getBoundingClientRect();
    const frame = panel.getBoundingClientRect();
    const offset = stripRunsAcross()
      ? box.left + box.width / 2 - frame.left
      : box.top + box.height / 2 - frame.top;
    panel.style.setProperty(ANCHOR_PROPERTY, `${Math.round(offset)}px`);
  };

  const syncPopover = () => {
    const card = !pinned;
    host.classList.toggle("rail-unpinned", !pinned);
    host.classList.toggle(COLLAPSED_CLASS, !panelVisible);
    host.classList.toggle(POPOVER_CLASS, card);
    const panel = host.querySelector("#rail-panel");
    if (panel) {
      panel.setAttribute("aria-hidden", String(!panelVisible));
      panel.toggleAttribute("inert", !panelVisible);
      anchorPopover(panel, card);
    }
    host.querySelectorAll(".rail-bubble").forEach((bubble) => {
      const expanded = panelVisible && bubble.classList.contains("active");
      bubble.setAttribute("aria-expanded", String(expanded));
      bubble.setAttribute("aria-controls", "rail-panel");
    });
  };

  /** Dock the panel, or let it go. Unpinning leaves the conversation on screen
   *  as the popover it becomes — the same move the inbox's pin makes
   *  (core/inboxShell.js) — unless the press was a way of shutting it. */
  const setPinned = (on) => {
    pinned = on;
    writePinned(on);
    panelVisible = true;
  };

  const activeBubble = () => host.querySelector(".rail-bubble.active");

  /** Collapse the live panel without changing its docked/card preference. */
  const closePanel = ({ restoreFocus = false } = {}) => {
    if (!panelVisible) return;
    const trigger = activeBubble();
    closeSurfaceMenu?.();
    closeSurfaceOverlay();
    panelMotion.setVisible({
      panel: host.querySelector("#rail-panel"),
      visible: false,
      apply: () => {
        panelVisible = false;
        paint();
      },
    });
    if (restoreFocus) trigger?.focus();
  };

  /** Outside press, Escape and navigation put an unpinned card away without
   * changing the remembered pin preference. */
  const dismissPopover = ({ restoreFocus = false } = {}) => {
    if (pinned || !panelVisible) return;
    closePanel({ restoreFocus });
  };

  const dismissOnEscape = (event) => {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    dismissPopover({ restoreFocus: true });
  };

  const dismissOnOutsidePointer = (event) => {
    if (pinned || !panelVisible || host.contains(event.target)) return;
    if (event.target.closest?.(".confirm-popover")) return;
    dismissPopover();
  };

  const shownPanelMode = () => (!agentInFocus() || !agentHasTerminal(agentInFocus()) ? "chat" : mode);

  const rememberedConversationIsLoading = () =>
    !addingAgent && !!selectedId && !isProvisionalKey(selectedId) && !agentInFocus();

  const panelBodyIdentity = () => {
    if (rememberedConversationIsLoading()) return "loading";
    if (addingAgent) return "new";
    return conversationKey();
  };

  const wantedPanelBody = () => `${shownPanelMode()}:${panelBodyIdentity()}`;

  const disposeTitleMotion = () => {
    titleMotion?.dispose();
    titleMotion = null;
  };

  const mountTitleMotion = (panel) => {
    const title = panel.querySelector(".rail-who");
    if (title) titleMotion = createChatTitleMotion(title);
  };

  const syncPanelTitle = (panel, fingerprint, title) => {
    if (panel.dataset.title === fingerprint) return;
    titleMotion?.show(title);
    panel.dataset.title = fingerprint;
  };

  const syncRemoveWording = (panel, removalWho) => {
    const remove = panel.querySelector(".rail-remove");
    if (!remove) return;
    const wanted = removeButtonTitle(removalWho);
    if (remove.title === wanted) return;
    remove.title = wanted;
    remove.setAttribute("aria-label", wanted);
  };

  const adoptPanelBody = (controller = null) => {
    const panel = host.querySelector("#rail-panel");
    if (panel) panel.dataset.body = controller
      ? `${shownPanelMode()}:${conversationKey(controller)}`
      : wantedPanelBody();
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 16, cap 10 — reduce it, then drop this line
  const paintPanel = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    const agent = agentInFocus();
    // The head wears the topic the agent named its work with, and a shimmering
    // "Starting" until it has. Hovering it says the topic in full — a head too
    // narrow for a long topic still gives it up on hover — and the harness name
    // while there is no topic to say. The harness icon beside it says which
    // harness either way.
    const who = agent ? agentWho(agent) : entity.kind === "issue" ? "Issue agent" : "New agent";
    const removalWho = agent ? agentRemovalWho(agent) : who;
    const heading = agent ? agentHeading(agent) : { text: who, starting: false };
    const provider = agent?.provider || "";
    const settled = settledAgentInFocus();
    const removable = canRemoveAgent({
      agents: visibleAgents(),
      agentId: settled ? settled.id : null,
      kind: entity.kind,
    });
    const hasTerminal = !!agent && agentHasTerminal(agent);
    // Which face this agent can actually wear. `mode` is remembered per work
    // item, so opening a terminal-less agent's bubble — or one whose digest
    // stopped offering a terminal under an open panel — arrives holding "tui"
    // for a screen that does not exist. The remembered choice is kept rather
    // than rewritten, so the agent beside it that does have a terminal is still
    // where the human left it.
    const shownMode = shownPanelMode();
    // Structural head changes replace its controls; a topic change keeps this
    // head mounted and moves only its title below — so the head is fingerprinted
    // by WHICH agent it is open on rather than by what that agent is called.
    // The id is what tells two starting agents on one harness apart: without it
    // a strip of two unnamed Claude Codes has one fingerprint, and switching
    // between them would leave the first one's head standing.
    const wantedHead = JSON.stringify([agent ? agent.id : "", provider, removable, hasTerminal]);
    const wantedTitle = JSON.stringify([who, heading.text, heading.starting]);
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = wantedPanelBody();
    if (panel.dataset.body !== wantedBody) {
      disposeTitleMotion();
      disposeTui();
      disposeSurfaces();
      disposeComposerClearance?.();
      disposeComposerClearance = null;
      closeSurfaceMenu?.();
      panel.innerHTML = `${panelHeadHtml(who, shownMode, { provider, removable, hasTerminal, surfaceOptions: surfaceMenuOptionsInFocus(), heading, pinned, removalWho })}
        <div class="rail-body" id="rail-body"></div>
        ${shownMode === "chat"
          ? `${rememberedConversationIsLoading() || entity.chatCapable === false ? "" : composerRowHtml()}`
          : ""}`;
      panel.dataset.head = wantedHead;
      panel.dataset.title = wantedTitle;
      panel.dataset.body = wantedBody;
      mountTitleMotion(panel);
      wireHead(panel);
      composerControl?.dispose?.();
      unsubscribeComposerController?.();
      unsubscribeComposerController = null;
      composerController = null;
      composerControl = null;
      composerModelMenu = null;
      if (shownMode === "tui") mountTui();
      else if (!rememberedConversationIsLoading() && entity.chatCapable !== false) {
        wireComposer(panel);
        deliverDroppedFiles();
        if (autofocusComposerPending) {
          autofocusComposerPending = false;
          panel.querySelector(`#${COMPOSER_IDS.input}`)?.focus();
        }
      }
    } else if (panel.dataset.head !== wantedHead) {
      // The agent's provider arrived after the fact, or the last agent beside
      // this one went away. Rewriting on every poll would eat a press that
      // landed mid-repaint.
      closeSurfaceMenu?.();
      disposeTitleMotion();
      panel.querySelector(".rail-head").outerHTML = panelHeadHtml(who, shownMode, {
        provider,
        removable,
        hasTerminal,
        surfaceOptions: surfaceMenuOptionsInFocus(),
        heading,
        pinned,
        removalWho,
      });
      panel.dataset.head = wantedHead;
      panel.dataset.title = wantedTitle;
      mountTitleMotion(panel);
      wireHead(panel);
    }
    syncPanelTitle(panel, wantedTitle, { text: heading.text, title: who, starting: heading.starting });
    // The remove button names the agent by its topic too, and a topic arriving
    // is not a structural change: it re-reads on the title's beat rather than
    // waiting for a head the rail has no reason to rebuild.
    syncRemoveWording(panel, removalWho);
    const pin = panel.querySelector(`.${PIN_CLASS}`);
    if (pin) syncPinButton(pin, { subject: PANEL_SUBJECT, pinned });
    paintSurfaceMenu();
    syncSurfaceOverlay();
    if (shownMode === "chat") {
      paintChat();
      paintRailStatus();
    }
  };

  const wireHead = (panel) => {
    const tuiToggle = panel.querySelector(".rail-tui");
    if (tuiToggle) {
      tuiToggle.onclick = () => {
        mode = mode === "tui" ? "chat" : "tui";
        railView.setPanelMode(mode);
        paintPanel();
      };
    }
    const remove = panel.querySelector(".rail-remove");
    if (remove) remove.onclick = () => removeAgent(remove);
    const pin = panel.querySelector(`.${PIN_CLASS}`);
    if (pin) pin.onclick = () => panelMotion.run({
      panel,
      direction: pinned ? "popover" : "pinned",
      scroller: panel.querySelector(".rail-body"),
      apply: () => {
        setPinned(!pinned);
        paint();
      },
    });
  };

  // ---- chat -----------------------------------------------------------------

  const threadWindow = () => {
    // The cache holds one conversation; switching bubbles switches which.
    if (threadAgentId !== selectedId) {
      resetConversationCache();
      threadAgentId = selectedId;
      bindConversationCache().seed(); // fire and forget; the refresh under way folds onto it
    }
    // The payload in hand belongs to the agent it was read for. Just after a
    // switch that is the agent just left, and absorbing it would refill the
    // cache the switch cleared with the wrong conversation — which is exactly
    // what made switching look like it did nothing. Until the read for THIS
    // agent lands (pressBubble asks for it immediately), the seeded window —
    // history the cache holds for this very agent — is what paints: a reader
    // in an active branch is never shown an empty frame the disk can fill.
    if (threadOwner !== selectedId) {
      const saved = threadCache.readWindow();
      return saved ? { items: saved.items, activityDigests: saved.activityDigests } : null;
    }
    if (!entity.thread) return null;
    // A payload folds through the cache once; a repaint of the same payload
    // renders the window the cache holds (what absorb would answer anyway).
    // Only a delta that never seated a window re-renders itself as it came.
    if (entity.thread === absorbedThreadPayload) {
      const held = threadCache.readWindow();
      return held
        ? { ...entity.thread, items: held.items, activityDigests: held.activityDigests }
        : { ...entity.thread };
    }
    absorbedThreadPayload = entity.thread;
    const thread = threadCache.absorb(entity.thread);
    bindConversationCache().persistThread();
    return thread;
  };

  const threadFor = () => {
    const scope = pendingThreadScope(selectedId);
    const thread = threadWindow();
    const held = (thread && thread.items) || [];
    reconcileOptimistic(scope, held, { keyOf: threadItemKey });
    const items = projectOptimistic(scope, held, { keyOf: threadItemKey });
    if (thread) return { ...thread, items };
    return items.length ? { items } : null;
  };

  /// Ask for the conversation above the window the reader is standing at the
  /// top of.
  ///
  /// A long conversation arrives as a page of its newest items — the wire
  /// carries a window, not a transcript — so the top of the scroller is a floor
  /// rather than the start, and this is what lifts it. One page in flight at a
  /// time: a scroll gesture fires the handler many times over, and each of
  /// those would otherwise be a round trip for the same history.
  const olderReadRequest = () => {
    if (!panelVisible) return null;
    const seek = threadCache.olderPageParam();
    if (loadingOlderItems || !seek || !threadCache.hasOlderItems() || !entity.entityId) return null;
    const call = chatRepository.currentCall();
    const addressed = controllerForAgent(agentOf(selectedId));
    return { seek, call, addressed, asked: selectedId };
  };

  const fetchOlderPage = async ({ call, addressed, seek, asked }) => {
    try {
      return await railContext.olderPage(call, {
        entityId: addressed?.identity.entityId || entity.entityId,
        agentId: addressed?.identity.agentId || asked,
        beforeSequence: seek.before_sequence,
      });
    } catch (error) {
      notifyError("Could not load older messages", error.message);
      return null;
    }
  };

  const readOlderItems = async () => {
    const request = olderReadRequest();
    if (!request) return;
    loadingOlderItems = true;
    const page = await fetchOlderPage(request);
    loadingOlderItems = false;
    if (!page || disposed || request.asked !== selectedId) return;
    if (threadCache.absorbOlderPage(page, request.seek)) {
      olderItemsAwaitingPaint = true;
      paintChat({ olderItemsPrepended: true });
    }
  };

  /// The chat tab of a work item with no agent: which harness to make one on,
  /// where the conversation would be. The composer below it is the live one —
  /// sending is what creates the highlighted agent and speaks to it.
  ///
  /// Rewritten only when the highlight moves, for the same reason the strip is:
  /// a poll that replaced these buttons would swallow the press landing on one.
  const paintNewAgent = (body) => {
    const chosen = newAgentChoice().provider;
    if (body.dataset.newAgent === chosen) return;
    const choices = creatable().providers.map((provider) =>
      `<button class="rail-harness-choice${provider.id === chosen ? " chosen" : ""}" type="button"
        data-provider="${esc(provider.id)}" aria-pressed="${provider.id === chosen}">
        ${harnessIconHtml(provider.id)}<span>${esc(provider.label)}</span>
      </button>`,
    ).join("");
    body.innerHTML = `<div class="rail-newagent">
      <p>Start a new conversation</p>
      <div class="rail-harness-picker" role="group" aria-label="Agent harness">${choices}</div>
    </div>`;
    body.dataset.newAgent = chosen;
    body.querySelector(".rail-newagent").onclick = (event) => {
      const card = event.target.closest(".rail-harness-choice");
      if (!card) return;
      if (card.dataset.provider === newAgentChoice().provider) {
        card.focus();
        return;
      }
      // A model belongs to its harness, so moving the highlight drops one
      // chosen under the harness beside it and brings the pressed harness's
      // own saved model and effort instead.
      writeNewAgentChoice(clampStoredAgentChoice(catalog, newAgentDefaultsFor(card.dataset.provider)));
      paintChat();
      body.querySelector(".rail-harness-choice.chosen")?.focus();
    };
  };

  /// The open runs of the conversation the panel is showing.
  ///
  /// One handle per conversation: switching bubbles is a different thread, with
  /// its own sequences and its own folds, so the runs the reader had open in
  /// the one they left do not follow them into the next.
  const conversationRuns = () => {
    const identity = controllerInFocus().identity;
    const runsFor = `${identity.entityId || ""}:${identity.agentId || ""}:${identity.conversationId || ""}`;
    if (!activityRuns || activityRunsFor !== runsFor) {
      activityRunsFor = runsFor;
      activityRuns = createActivityRuns({
        deviceId: cacheScope?.deviceId,
        entityId: identity.entityId,
        agentId: identity.agentId,
        call: (method, params) => chatRepository.currentCall()(method, params),
      });
      // Everything else the panel remembers about the conversation goes with
      // it: a line ruled in one thread marks nothing in the next, and how far
      // this panel read one says nothing about the other.
      unreadFrom = null;
      reportedRead = 0;
      reportedFloor = null;
    }
    return activityRuns;
  };

  /// Where the unread line stands in the conversation on screen, and with it
  /// where a paint that is following the conversation lands.
  ///
  /// The daemon's cursor says how far the reader got and the bubble's count says
  /// whether anything is waiting past it; core/unreadAnchor.js reads those two
  /// as a place. It is HELD rather than recomputed, because a message is read
  /// the moment its foot comes into view — a line taken from the live cursor
  /// alone would rule itself above what just arrived and clear itself a tick
  /// later. The reader reaching the end with nothing waiting is what retires it.
  const unreadLineFor = (body, thread) => {
    const agent = agentInFocus();
    return unreadAnchorSequence({
      held: unreadFrom,
      cursor: agent ? agent.read_through_sequence : undefined,
      unreadCount: (agent && agent.unread_count) || 0,
      items: threadItems(thread),
      caughtUp: isAtBottom(body),
    });
  };

  /// What the daemon last said each run over this window totals: the digests
  /// the timeline on screen was drawn with.
  ///
  /// The paint is what holds them rather than the cache, because the two part
  /// company — a tick that lets a broken window go still paints the rows and
  /// digests it was drawn from — and a run is pressed on the timeline the
  /// reader is looking at, not on the window behind it.
  const digestsInHand = () => paintedDigests;

  /// The items fetched for a run, by the key the fold names it with — the runs
  /// hold the span each fetch covered, so there is nothing to translate here.
  const fetchedRunItems = (runKey) => conversationRuns().itemsOf(runKey);

  /// The newest sequence the conversation has reached — the end a run has to
  /// touch to be the live tail.
  const lastSequenceOf = (thread) => (thread && thread.thread_last_sequence) || 0;

  /// How far the window has taken delivery of the conversation. The one number
  /// that moves for an item mutated in place — a call answered, a message
  /// marked seen — which changes what a row says without changing how many
  /// rows there are.
  const deliveredSequenceOf = (thread) => {
    const held = threadCache.readWindow();
    return Math.max(held ? held.deliveredSequence : 0, lastSequenceOf(thread));
  };

  const threadItems = (thread) => (thread && thread.items) || [];

  const chatFingerprintOf = (thread, agentLabel) => {
    const openRuns = conversationRuns().openKeys();
    const threadState = controllerInFocus().threadState;
    return chatPaintFingerprint({
      deliveredSequence: deliveredSequenceOf(thread),
      itemCount: threadItems(thread).length,
      digests: digestsOf(thread),
      openRunKeys: openRuns,
      fetchedRunKeys: [...openRuns].filter((key) => fetchedRunItems(key)),
      selectedAgentId: selectedId,
      agentLabel,
      unreadFrom,
      ...threadOfferState(threadState),
    });
  };

  /// The conversation, unless nothing it is drawn from has moved.
  ///
  /// Everything else the panel shows — the composer, the pills, the read report
  /// — is about the agent rather than about what it said, so a skipped timeline
  /// never skips those.
  const paintTimeline = (body, thread, olderItemsPrepended) => {
    const agentLabel = providerLabel((agentOf(selectedId) || {}).provider);
    // Whose conversation this is, settled first: a switch drops everything the
    // panel remembers about the last one, including the line about to be ruled.
    const runs = conversationRuns();
    // Measured before the paint, because where the reader is standing NOW is
    // what says whether they have caught up.
    unreadFrom = unreadLineFor(body, thread);
    const fingerprint = chatFingerprintOf(thread, agentLabel);
    if (fingerprint === paintedChat && body.querySelector(".thread-items")) return;
    paintedChat = fingerprint;
    paintedDigests = digestsOf(thread);
    const built = timelineEntries(threadItems(thread), agentLabel, thread && thread.id, paintedDigests, {
      openRuns: runs.openKeys(),
      runItemsOf: fetchedRunItems,
      threadState: controllerInFocus().threadState,
      unreadFrom,
    });
    // No composer in here: the box is pinned below this scroller, so what the
    // poll repaints is the timeline and only the timeline.
    timedPaint("chat", () =>
      paintThreadKeepingPlace(body, () => {
        paintThreadEntries(body, built);
        wireTimeline(body);
      }, { olderItemsPrepended }),
    );
  };

  const paintChat = ({ olderItemsPrepended = false } = {}) => {
    if (!panelVisible) return;
    const body = host.querySelector("#rail-body");
    if (!body) return;
    if (entity.chatCapable === false) {
      body.innerHTML = '<div class="rail-chat-loading">This workspace does not have an agent conversation yet.</div>';
      return;
    }
    if (rememberedConversationIsLoading()) {
      body.innerHTML = '<div class="rail-chat-loading">Loading chat…</div>';
      syncSurfaces();
      return;
    }
    if (!visibleAgents().length || addingAgent) {
      paintNewAgent(body);
      syncComposer();
      syncSurfaces();
      return;
    }
    const prepended = olderItemsPrepended || olderItemsAwaitingPaint;
    paintTimeline(body, threadFor(), prepended);
    olderItemsAwaitingPaint = false;
    // Assignment rather than a listener: the scroller outlives every repaint,
    // and adding one per paint would ask for the same page once per tick.
    body.onscroll = () => {
      if (!panelVisible) return;
      if (body.scrollTop <= OLDER_ITEMS_TRIGGER_PX) readOlderItems();
      reportRead(body);
    };
    syncComposer();
    syncSurfaces();
    reportRead(body);
  };

  /// The reader pressing a folded run, settled: the fold has flipped, and
  /// whatever the new side needed fetching has landed.
  ///
  /// The repaint is what draws the box open or shut — the press's own
  /// activation is cancelled where it is wired, so `open` says what this says.
  /// A run the window holds whole is drawn from the window, and one the page cut
  /// is asked for; the daemon holds the half that never travelled.
  ///
  /// The pressed box says how far the run reaches on this side, which is what
  /// keeps the live tail out of the cache: its digest stops where the last page
  /// cut it, and every delta since has landed on the run without moving it.
  const pressActivityRun = async (runKey) => {
    const runs = conversationRuns();
    const runThrough = activityRunThroughAt(host.querySelector("#rail-body"), runKey);
    const opened = runs.toggle(runKey);
    // Decided against the timeline the press landed on, before the repaint
    // draws the next one over it.
    const digest = opened
      ? runDigestToFetch(digestsInHand(), runKey, lastSequenceOf(threadFor()), runThrough)
      : null;
    paintChat();
    if (!digest) return;
    const filled = await runs.open(digest).catch((error) => {
      notifyError("Could not load this activity", error.message);
      return false;
    });
    if (filled && !disposed) paintChat();
  };

  const composerPlaceholder = () =>
    visibleAgents().length && !addingAgent ? "Send a message to this agent…" : "Send a message to start an agent here…";

  /// The box you write in, pinned below the conversation instead of sitting at
  /// the end of it. It is a SIBLING of the scroller, so reading back through a
  /// long thread never takes the box off screen, and the growth of a box being
  /// typed into comes out of the thread above rather than pushing its own
  /// bottom edge past the panel.
  const composerRowHtml = () =>
    `<div class="rail-composer" id="rail-composer">
      ${railViewerHostHtml()}
      ${railObservationHostHtml()}
      ${railStatusRowHtml()}
      <div class="chat-recovery" id="rail-chat-recovery"></div>
      ${composerHtml({
        inputId: COMPOSER_IDS.input,
        sendId: COMPOSER_IDS.send,
        hintId: COMPOSER_IDS.hint,
        placeholder: composerPlaceholder(),
        attachable: true,
        modelMenu: true,
        canInterrupt: agentCanInterrupt(agentOf(selectedId)),
      })}</div>`;

  /// The one thing on the composer a poll can change: whether the send offers
  /// to stop the turn in flight, which moves every time an agent starts or
  /// finishes one. The box itself is never rebuilt for it — a rebuild would
  /// take the draft and the focus with it, mid-sentence. (The placeholder
  /// cannot move under a poll: it says whether there is an agent to talk to,
  /// and gaining one rebuilds the panel around a conversation.)
  /** The agent the panel is about — none while the chooser is up, whatever
   *  bubble is technically still selected behind it. */
  const agentInFocus = () => {
    if (addingAgent) return null;
    return entity.executionContext?.agent || agentOf(selectedId);
  };

  const settledAgentInFocus = () => {
    const agent = agentInFocus();
    return agent && !isProvisionalKey(agent.id) ? agent : null;
  };

  const composerDisplayChoice = (agent, settled) => {
    if (!settled) return newAgentChoice();
    return {
      provider: settled.provider || agent?.provider || chosenProviderId(creatable(), NO_AGENT_CHOICE),
      model: settled.requestedModel,
      effort: settled.effort,
    };
  };

  const syncComposer = () => {
    if (composerControl) composerControl.setCanInterrupt(agentCanInterrupt(agentInFocus()));
    if (composerControl) composerControl.setBlocked(composerController?.choice().pending, "Applying model…");
    syncChatRecovery();
    if (!composerModelMenu) return;
    const agent = agentInFocus();
    const settled = composerController?.choice();
    const choice = composerDisplayChoice(agent, settled);
    composerModelMenu.set(catalog, choice.provider, choice, settled?.activeModel || "", settled?.activeEffort || "");
  };

  const syncChatRecovery = () => {
    const recoveryHost = host.querySelector("#rail-chat-recovery");
    if (!recoveryHost) return;
    const controller = composerController || controllerInFocus();
    setMotionRowHtml(recoveryHost, chatRecoveryHtml(controller));
    recoveryHost.querySelectorAll(".chat-recovery-entry button").forEach((button) => {
      button.onclick = async () => {
        const operationId = button.closest("[data-operation]").dataset.operation;
        button.disabled = true;
        try {
          await controller.retryOperation(operationId, {
            retryCreation: (submission) => {
              createAgentWithMessage(controller, submission, { retry: true });
              return creating?.promise;
            },
          });
          await refresh();
        } catch (error) {
          notifyError("Could not retry the message", error.message);
          syncChatRecovery();
        }
      };
    });
  };

  /** What the composer's model menu is editing: the open agent's own choice —
   *  its harness names the catalog — or, before there is an agent, what the
   *  first send will create one with. */
  const composerChoice = () => {
    const agent = agentInFocus();
    if (!agent) return newAgentChoice();
    const choice = controllerForAgent(agent).choice();
    return { provider: choice.provider || agent.provider, model: choice.requestedModel, effort: choice.effort };
  };

  const activeModelOf = (agent) => (agent ? controllerForAgent(agent).choice().activeModel : "");

  /** A model or effort picked from that menu.
   *
   *  With an agent it is the entity's persisted choice, which the NEXT start
   *  spends — the session running right now is never touched, which is what
   *  makes the menu safe to press mid-turn. With none there is nothing on the
   *  bridge to write to yet, so it waits for the send that creates one. */
  const chooseModel = async (next) => {
    const agent = settledAgentInFocus();
    if (!agent) {
      writeNewAgentChoice(next);
      return;
    }
    const controller = controllerForAgent(agent);
    const choosing = controller.chooseModel(next);
    await runOptimistic({
      scope: pendingAgentsScope(),
      records: [patchRecord(agent.id, { model: next.model, effort: next.effort })],
      call: () => choosing,
      failureSummary: "Could not set the model",
    });
    await refresh();
  };

  const wireTimeline = (body) => {
    const controller = controllerInFocus();
    wireExpansionReveal(body);
    wireThreadAttachments(
      body,
      (path) => chatRepository.currentCall()("thread.attachment", { entity_id: controller.identity.entityId, path }),
      controller.threadState,
    );
    wireThreadRevisionLinks(body, (revisionId) =>
      chatRepository.currentCall()("thread.revision", { entity_id: controller.identity.entityId, revision_id: revisionId }),
    );
    wireThreadLinks(body, openLink, routeForLink);
    wireThreadOptions(body, (choice) => choose(choice).catch((error) => {
      notifyError("Choice failed", error.message);
      throw error;
    }), controller.threadState);
    // Delegated, because the row a press lands on is redrawn under it: the
    // scroller outlives every repaint, and the run names itself on the head.
    //
    // The default is cancelled so the browser never toggles the `<details>`
    // itself. It would do so AFTER this handler, undoing whatever the repaint
    // wrote, and a run that opens onto rows it has to fetch cannot be a fold
    // two writers share.
    body.onclick = (event) => {
      const runKey = pressedActivityRunKey(event.target);
      if (!runKey) return;
      event.preventDefault();
      pressActivityRun(runKey);
    };
  };

  /// Wire the pinned box. `panel` rather than the composer row itself, so a file
  /// dropped anywhere on the conversation lands in the tray — the gesture aims
  /// at the agent, not at a 40px strip.
  const wireComposer = (panel) => {
    const controller = controllerInFocus();
    composerController = controller;
    const binding = controller.bindDraft();
    composerControl = wireThreadComposer(panel, {
      ids: COMPOSER_IDS,
      readDraft: binding.readDraft,
      writeDraft: binding.writeDraft,
      readAttachments: binding.readAttachments,
      writeAttachments: binding.writeAttachments,
      submissionOwnsDraft: true,
      viewingContext: context.viewingContext || App.viewingContext,
      // Attaching lands the bytes before the message names them — which needs a
      // conversation to store them against, so it adopts exactly as sending
      // does: choosing a file for a message is the same intent, one keystroke
      // earlier.
      upload: async (file, contentBase64) => {
        const call = chatRepository.currentCall();
        const entityId = await ensureEntity(call);
        return call("thread.attach", { entity_id: entityId, filename: file.name, content_b64: contentBase64 });
      },
      onSubmit: (message, attachments, options) => sendFrom(controller, message, attachments, options),
      onInterrupt: () => {
        const { entityId, agentId, conversationId } = controller.identity;
        return chatRepository.currentCall()("agent.interrupt", {
          entity_id: entityId,
          agent_id: agentId,
          conversation_id: conversationId,
        });
      },
      onError: (error) => notifyError("Message failed", error.message),
    });
    composerModelMenu = mountComposerModelMenu(panel, { ids: COMPOSER_IDS, onChoose: chooseModel });
    unsubscribeComposerController?.();
    unsubscribeComposerController = controller.subscribe(syncComposer);
    mountSurfaces(panel);
    syncComposer();
    syncSurfaces();
    disposeComposerClearance = mountComposerClearance(panel);
  };

  const surfaceModelLabel = (modelId) => {
    const agent = agentInFocus();
    return activeModelLabel(catalog, agent ? agent.provider : "", modelId);
  };

  /// A reference from a surface points at a call, and a call folded into a shut
  /// run has no row to point at — so the run it sits in is opened first, and
  /// waited for: the half of a cut run the window never held is a fetch away,
  /// and reaching for the row before it lands finds nothing.
  const openRunHolding = async (body, sequence) => {
    const runKey = activityRunKeyAt(body, sequence);
    if (!runKey || conversationRuns().isOpen(runKey)) return;
    await pressActivityRun(runKey);
  };

  const surfaceViewerCallbacks = () => ({
    modelLabel: surfaceModelLabel,
    onOpenThreadItem: async (sequence) => {
      const body = host.querySelector("#rail-body");
      await openRunHolding(body, sequence);
      if (revealThreadSequence(body, sequence)) return;
      notifyError(
        "That call is not in the loaded conversation",
        "Scroll back to load older items, then press the row again.",
      );
    },
  });

  const surfacesSeen = () => {
    const agent = agentInFocus();
    const live = agent && agent.surfaces;
    const generation = surfaceSessionGeneration(agent?.surface_session_generation);
    if (live) return { surfaces: live, generation, at: Date.now() };
    if (agent && seededSurfaces?.generation === generation) return seededSurfaces;
    return { surfaces: null, generation, at: Date.now() };
  };

  const surfaceMenuOptionsInFocus = () => surfaceMenuOptions(surfacesSeen().surfaces);

  const mountSurfaces = (panel) => {
    const pillHost = panel.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const viewerHost = panel.querySelector(`#${RAIL_VIEWER_ID}`);
    const observationHost = panel.querySelector(`#${RAIL_OBSERVATION_ID}`);
    if (!pillHost || !viewerHost || !observationHost) return;
    observationBlock = mountAgentObservation(observationHost);
    surfacesBlock = mountAgentSurfaces({
      pillHost,
      viewerHost,
      key: conversationKey(),
      onPillsChanged: syncRailStatusRow,
      ...surfaceViewerCallbacks(),
    });
  };

  const disposeSurfaces = () => {
    closeSurfaceOverlay();
    observationBlock?.dispose();
    observationBlock = null;
    if (!surfacesBlock) return;
    surfacesBlock.dispose();
    surfacesBlock = null;
  };

  const syncSurfaces = () => {
    if (!surfacesBlock || !observationBlock) return;
    const seen = surfacesSeen();
    surfacesBlock.set(seen.surfaces, seen.at);
    observationBlock.set(seen.surfaces, {
      generation: seen.generation,
      working: agentInFocus()?.working === true,
    });
  };

  const paintSurfaceMenu = () => {
    const region = host.querySelector(SURFACE_MENU_SELECTOR);
    if (!region) return;
    closeSurfaceMenu = mountMenuIfChanged(region, surfaceMenuHtml(surfaceMenuOptionsInFocus()), {
      onChoose: openSurfaceOverlayForKind,
    });
  };

  const openSurfaceOverlayForKind = (kind) => {
    closeSurfaceOverlay();
    surfaceOverlay = openSurfaceOverlay(kind, {
      ...surfaceViewerCallbacks(),
      host: host.querySelector("#rail-panel"),
      onClose: () => {
        surfaceOverlay = null;
      },
    });
    syncSurfaceOverlay();
  };

  const syncSurfaceOverlay = () => {
    if (!surfaceOverlay) return;
    surfaceOverlay.set(surfacesSeen().surfaces);
  };

  const closeSurfaceOverlay = () => {
    if (!surfaceOverlay) return;
    surfaceOverlay.close();
    surfaceOverlay = null;
  };

  /** What the paths in this conversation are written against: the checkout the
   *  rail is mounted on, and — for a workspace — the directories it is made of,
   *  so a path mounted under one of them opens in that directory. */
  const linkContext = () => ({
    kind: entity.kind,
    deviceId: context.deviceId ?? null,
    projectId: entity.projectId || context.projectId,
    workspaceId: context.workspaceId,
    sourceId: context.sourceId,
    directories: entity.directories,
    branch: entity.branch,
  });

  /** Where a file reference points, for the chip's own href and for the press
   *  on it alike — one answer, so a link says where it goes. */
  const routeForLink = (link) => fileLinkRoute(link, linkContext());

  /** A reference in the conversation goes where it points, as far as the two
   *  work-item surfaces can take it. */
  const openLink = (link) => {
    if (link.issue_id || link.plan_id) {
      go({ name: "issue", projectId: entity.projectId, id: link.issue_id || link.plan_id });
      return;
    }
    const route = routeForLink(link);
    if (route) go(route);
  };

  /// Tell the daemon how much of this agent's conversation has been read, and
  /// how much of it this panel was ever sent.
  ///
  /// Reading is per MESSAGE: what the reader's viewport reached is what clears,
  /// so a glance at the top of a long thread clears the top of it and the rest
  /// goes on waiting. The end of the scroller is the end of a WINDOW, though —
  /// a long conversation arrives as a page of its newest items — so the floor
  /// of that window goes with the report. Without it the daemon reads the whole
  /// conversation through, clearing the badge for a message waiting a hundred
  /// items back that this panel never received and nobody ever saw.
  ///
  /// A scroll gesture fires this many times over, so a report that says what
  /// the last one said is never made.
  const reportRead = (body) => {
    if (!panelVisible) return;
    const agent = agentInFocus();
    if (!agent || !agent.unread_count) return;
    const controller = controllerForAgent(agent);
    if (!controller.identity.entityId || !controller.identity.agentId) return;
    const read = readThroughSequence(body);
    const floor = threadCache.windowFloorSequence();
    if (!readingIsNews(read, floor)) return;
    reportedRead = read;
    reportedFloor = floor;
    markSeen(controller.identity.entityId, controller.identity.agentId, floor, read).then(refreshFeed);
  };

  /// Whether a read report says anything the last one did not.
  ///
  /// Two things can make it news. The reader got further down the conversation,
  /// which is the ordinary case. Or the window they hold reaches further back —
  /// a report the daemon dropped because it could not vouch for the history
  /// under the floor is worth making again once that history has landed.
  const readingIsNews = (read, floor) =>
    read > reportedRead || (typeof floor === "number" && floor < (reportedFloor ?? Infinity));

  // ---- sending --------------------------------------------------------------

  /** The entity a message is posted to, adopting the checkout first when Build
   *  owns nothing here yet — an agent needs an owner for `done` to report to. */
  const ensureEntity = async (call) => {
    if (entity.entityId && !entity.adoptable) return entity.entityId;
    if (!ensuringConversation) {
      const ensureConversation = railContext.ensureConversation(call);
      if (ensureConversation) {
        ensuringConversation = Promise.resolve(ensureConversation).then((answer) => {
          const entityId = answer?.entity_id || answer?.run_id;
          if (!entityId) throw new Error("workspace.ensure_conversation did not return an entity id");
          entity = { ...entity, entityId, chatCapable: true };
          return entityId;
        }).catch((error) => {
          ensuringConversation = null;
          throw error;
        });
      }
    }
    if (ensuringConversation) {
      return ensuringConversation;
    }
    const adopt = adoptingCall(call);
    if (!adopt) return entity.entityId;
    return adopt.adopt();
  };

  const repaintComposerFromDraft = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    panel.dataset.body = "";
    paintPanel();
  };

  const renameAgentIdentity = (fromAgentId, toAgentId, controller) => {
    if (!fromAgentId || !toAgentId || fromAgentId === toAgentId) return;
    const renamedAgentIsSelected = selectedId === fromAgentId;
    if (threadAgentId === fromAgentId) threadAgentId = toAgentId;
    if (threadOwner === fromAgentId) threadOwner = toAgentId;
    const fromFaceKey = faceKey("agent", fromAgentId);
    const toFaceKey = faceKey("agent", toAgentId);
    const face = faces.get(fromFaceKey);
    if (face) {
      faces.set(toFaceKey, face);
      faces.delete(fromFaceKey);
    }
    const strip = host.querySelector(".rail-strip");
    if (strip) rekeyEntry(strip, fromFaceKey, toFaceKey);
    if (renamedAgentIsSelected) chooseAgent(toAgentId);
    adoptPanelBody(renamedAgentIsSelected ? controller : null);
  };

  const provisionalMessageEntry = (messageKey, message) => ({
    type: "message",
    data: {
      role: "user",
      sequence: messageKey,
      body: message.body || "",
      attachments: message.attachments || [],
      created_at: new Date().toISOString(),
      delivery_status: "queued",
    },
  });

  const provisionalDeliveryStatus = (submission, posted) => {
    if (posted?.operation_status === "uncertain") return "uncertain";
    return submission.threadPostOperations ? "queued" : "sent";
  };

  const rekeyPostedMessage = (handle, messageKey, provisional, posted, submission) => {
    const sequence = (posted && posted.posted_sequence) ?? null;
    if (sequence === null) handle.drop(messageKey);
    else handle.rekey(messageKey, String(sequence), {
      ...provisional,
      data: { ...provisional.data, sequence, delivery_status: provisionalDeliveryStatus(submission, posted) },
    });
  };

  /** Put the message on the conversation, and settle the provisional row under
   *  the sequence the daemon gave it.
   *
   *  A post the browser stopped waiting for is not a refusal: the turn is
   *  durable on the daemon's side, and reverting it here would hand the draft
   *  back and have the human send the same turn twice. The provisional row
   *  stands instead, and the next thread read replaces it with the real
   *  message. */
  const postMessage = async (handle, { controller, submission, messageKey, provisionalMessage }) => {
    let posted;
    try {
      posted = await replyOrNothing(controller.post(submission, MUTATION_THREAD_PAGE));
    } catch (error) {
      if (!error.uncertain) throw error;
      posted = null;
    }
    if (posted) rekeyPostedMessage(handle, messageKey, provisionalMessage, posted, submission);
  };

  /** Put an agent on this entity's message, and say which agent got it.
   *
   *  The daemon answers a start before the harness exists and need not name the
   *  agent it opened; the entity does, on its next answer, so a reply without
   *  one — or no reply at all, when the start outlives the timer — is read
   *  there instead. */
  const wakeAgent = async (submission) => {
    const { entityId, agentId } = submission.address;
    const started = await replyOrNothing(submission.call("agent.start", { id: entityId, agent_id: agentId }));
    if (started && started.agent_id && started.agent_id !== agentId) {
      throw new Error("agent.start answered for a different agent");
    }
    return agentId;
  };

  /** The state a row wears from the moment a session is asked for until the
   *  entity answers — one record, whichever verb asked: the Resume press or
   *  the message that wakes the agent behind it. */
  const startingRecord = (agentId) =>
    patchRecord(agentId, { state: AGENT_STARTING }, { scope: pendingAgentsScope(), clearedBy: agentSessionAnswered });

  const createAgentWithMessage = (controller, submission, { retry = false } = {}) => {
    const provisionalAgentId = provisionalKey("agent");
    const provisionalMessageKey = provisionalKey("message");
    const selectedBeforeCreate = selectedId;
    const addingBeforeCreate = addingAgent;
    const choice = submission.creationChoice;
    const provisionalAgent = {
      id: provisionalAgentId,
      ordinal: visibleAgents().length + 1,
      provider: choice.provider,
      model: choice.model || "",
      effort: choice.effort || "",
      state: "idle",
      unread_count: 0,
      unread_reason: null,
      working: false,
      has_terminal: false,
    };
    const provisionalMessage = provisionalMessageEntry(provisionalMessageKey, submission.message);
    addingAgent = false;
    openConversation(provisionalAgentId);
    adoptPanelBody();
    let messageDelivered = false;

    const creationCall = retry ? chatRepository.currentCall() : submission.call;
    controller.markOperationKind(submission, "creation");
    const call = async (handle) => {
      const entityId = await ensureEntity(creationCall);
      const added = await creationCall("agent.add", {
        entity_id: entityId,
        creation_id: submission.creationId,
        ...newAgentParams(choice),
      });
      const createdAgent = (added && added.agent) || null;
      if (!createdAgent || !createdAgent.id) throw new Error("agent.add did not return the created agent");
      const resolvedIdentity = {
        entityId,
        agentId: createdAgent.id,
        conversationId: conversationIdOf(createdAgent),
      };
      chatOwnership.resolve(controller, resolvedIdentity);
      controller.absorbAgent(createdAgent);
      controller.clearOperationKind(submission);
      handle.moveScope(pendingThreadScope(provisionalAgentId), pendingThreadScope(createdAgent.id));
      renameAgentIdentity(provisionalAgentId, createdAgent.id, controller);
      handle.rekey(provisionalAgentId, createdAgent.id, { ...provisionalAgent, ...createdAgent, id: createdAgent.id });
      const addressedSubmission = controller.addressSubmission(submission, creationCall);
      await postMessage(handle, {
        controller,
        submission: addressedSubmission,
        messageKey: provisionalMessageKey,
        provisionalMessage,
      });
      messageDelivered = true;
      await wakeAgent(addressedSubmission);
    };

    const onRevert = (error) => {
      if (operationIsUncertain(error)) {
        controller.recordOperationFailure(submission, error);
        if (selectedId === provisionalAgentId) {
          chooseAgent(selectedBeforeCreate);
          addingAgent = true;
          paint();
        }
        return;
      }
      if (isProvisionalKey(selectedId)) {
        addingAgent = addingBeforeCreate;
        openConversation(selectedBeforeCreate);
      }
      if (messageDelivered) return;
      const restored = controller.restoreRejected(submission, error);
      if (controllerInFocus() !== controller) return;
      if (restored === "restored") repaintComposerFromDraft();
      else syncChatRecovery();
    };

    const settling = runOptimistic({
      scope: pendingAgentsScope(),
      records: [
        insertRecord(provisionalAgentId, provisionalAgent),
        insertRecord(provisionalMessageKey, provisionalMessage, {
          scope: pendingThreadScope(provisionalAgentId),
        }),
      ],
      call,
      failureSummary: "Could not start the agent",
      onRevert,
    }).then(async () => {
      await refreshFeed();
      await refresh();
    });
    creating = { controller, promise: settling };
    settling.finally(() => {
      if (creating && creating.promise === settling) {
        creating = null;
        if (controller.isBound()) chatOwnership.releaseProvisional();
      }
    });
    return undefined;
  };

  const deliverSubmission = (controller, submission) => {
    const messageKey = provisionalKey("message");
    const provisionalMessage = provisionalMessageEntry(messageKey, submission.message);
    const addressedAgentId = submission.address.agentId;
    const agent = agentOf(addressedAgentId);
    const wakesAgent = (entity.kind === "branch" || entity.kind === "workspace") && !agentIsUp(agent);

    let messageDelivered = false;

    const call = async (handle) => {
      await postMessage(handle, { controller, submission, messageKey, provisionalMessage });
      messageDelivered = true;
      if (wakesAgent) {
        await wakeAgent(submission);
      }
    };

    runOptimistic({
      scope: pendingThreadScope(addressedAgentId),
      records: [
        insertRecord(messageKey, provisionalMessage),
        ...(wakesAgent && agent ? [startingRecord(agent.id)] : []),
      ],
      call,
      failureSummary: "Message failed",
      onRevert: (error) => {
        if (messageDelivered) return;
        const restored = controller.restoreRejected(submission, error);
        if (controllerInFocus() !== controller) return;
        if (restored === "restored") repaintComposerFromDraft();
        else syncChatRecovery();
      },
    }).then(async () => {
      await refreshFeed();
      await refresh();
    });
    paintChat();
    return undefined;
  };

  const postProvisional = (controller, submission) => {
    if (!creating) return createAgentWithMessage(controller, submission);
    if (creating.controller !== controller) throw new Error("Another agent creation is already in flight");
    creating.promise.then(() => {
      const addressed = controller.addressSubmission(submission);
      deliverSubmission(controller, addressed);
    });
    return undefined;
  };

  /** A typed message. `interrupt` rides on the post rather than travelling as a
   *  verb of its own: Build never stops a turn without one to put in its place,
   *  and a second round trip is a window in which the agent starts a fresh turn
   *  or finishes. One send path, one flag. */
  const sendFrom = (controller, body, attachments, { interrupt = false } = {}) => {
    const message = { body, attachments, ...(interrupt ? { interrupt: true } : {}) };
    if (!controller.identity.agentId) {
      const submission = controller.captureProvisionalSubmission(message, newAgentChoice());
      return postProvisional(controller, submission);
    }
    return deliverSubmission(controller, controller.captureSubmission(message));
  };

  /** A press on the actions the agent suggested. It goes out as the message it
   *  is — same adoption, same waking, same refresh — and the daemon composes
   *  what the agent hears out of the options it offered. */
  const choose = ({ messageId, optionIds }) => {
    const controller = controllerInFocus();
    const message = { option_reply: { message_id: messageId, option_ids: optionIds } };
    return deliverSubmission(controller, controller.captureSubmission(message));
  };

  // ---- the strip's presses --------------------------------------------------

  /** A press that moves the rail between its two conversations, or null when it
   *  is not one: the project's bubble from the work item, and any bubble below
   *  the line from the project's — which is the way back. */
  const swapForPress = (type, agentId) => {
    if (!swap) return null;
    if (type === "project" && !onProjectAgentRail) return () => openProjectAgent();
    if (onProjectAgentRail && (type === "agent" || type === "ghost")) {
      return () => swap.toWorkItem(agentId || null);
    }
    return null;
  };

  /// The owner `project.ensure_conversation` answers with. The press names no
  /// harness, model or effort: what a project agent starts on is the DEVICE's
  /// setting, and the bridge mints the owner on it.
  const mintProjectConversation = async () => {
    const call = chatRepository.currentCall();
    const answer = await call("project.ensure_conversation", {
      project_id: projectAgent.projectId,
    });
    return answer?.entity_id || answer?.run_id;
  };

  /**
   * Put the project's conversation in this panel, with the page staying on the
   * work item it is standing on.
   *
   * The owner is minted HERE, by the press — a rail that minted one to paint a
   * bubble would give every workspace page a project agent, a scratch directory
   * and a run that nobody asked for.
   */
  const openProjectAgent = async () => {
    if (projectOwner) {
      swap.toProject(projectOwner);
      return;
    }
    if (mintingProjectAgent) return;
    mintingProjectAgent = true;
    try {
      const entityId = await mintProjectConversation();
      if (!disposed && entityId) swap.toProject(entityId);
    } catch (error) {
      mintingProjectAgent = false;
      if (!disposed) notifyError("No conversation for this project", error.message || String(error));
    }
  };

  const pressBubble = (type, agentId) => {
    if (type === "add") {
      pressAddBubble();
      return;
    }
    const swapping = swapForPress(type, agentId);
    if (swapping) {
      swapping();
      return;
    }
    if (type === "agent" && agentId && (agentId !== selectedId || addingAgent)) {
      openAgent(agentId);
      paint();
      // …and ask for this agent's conversation NOW. Waiting for the watcher is
      // what made the switch look broken: with the bridge pushing change events
      // the rail's own read has stood down to a 60s safety poll, and nothing
      // about opening a different bubble is a change the bridge would push.
      refresh();
      return;
    }
    // The bubble already open is the way back out: press it again to put the
    // panel away, whichever way it is on the screen.
    if (panelOut()) closePanel({ restoreFocus: true });
    else showPanel();
  };

  /** Put the panel on screen without touching the pin: docked it is already
   *  there, and unpinned this is the popover opening on the bubble that was
   *  pressed. */
  const showPanel = () => {
    if (panelVisible) return;
    const opening = !host.querySelector("#rail-panel");
    panelVisible = true;
    paint();
    const standing = host.querySelector("#rail-panel");
    panelMotion.setVisible({ panel: standing, visible: true, opening, apply: syncPopover });
  };

  /** Open this agent's conversation in the panel, with the panel out. */
  const openAgent = (agentId) => {
    addingAgent = false; // opening a real conversation ends the chooser
    openConversation(agentId);
    showPanel();
  };

  /** Another agent on this branch, beside the ones already here. It starts on
   *  the harness this browser prefers (an empty preference sends nothing and
   *  the daemon's own default stands) and nothing runs until it is spoken to.
   *
   *  The preference is a record and the offer is where it clamps: a browser
   *  that stored the carrier no surface offers any more creates the agent every
   *  other surface would have created. */
  const pressAddBubble = () => {
    if (!entity.entityId) return;
    addingAgent = !addingAgent;
    if (addingAgent) {
      // The browser's stored harness preference seeds the chooser's highlight,
      // clamped to the offer — the record the silent + used to spend outright.
      // The human now sees the choice before anything is created; the send is
      // what creates, exactly as it does on a branch with no agents at all.
      seedNewAgentDefaults();
      showPanel();
      disposeTui();
    }
    paint();
  };

  /**
   * Take the open agent back off the branch.
   *
   * Destructive twice over — the agent's session is killed and its conversation
   * goes with it — so it asks first, with the outline of what will happen.
   *
   * A daemon that refuses (an older binary with no `agent.remove` at all, or an
   * agent whose session is mid-spawn) leaves the rail exactly as it was: the
   * refusal is said the standard way and the same button is still there to try
   * again with.
   */
  const removeAgent = async (anchor) => {
    const agent = settledAgentInFocus();
    if (!agent || !entity.entityId) return;
    const entityId = entity.entityId;
    const call = chatRepository.currentCall();
    if (!(await confirmActionAt(anchor, removeAgentConfirm(agent, entity.kind)))) return;
    if (disposed || entity.entityId !== entityId || settledAgentInFocus()?.id !== agent.id) return;
    if (isPending(pendingAgentsScope(), agent.id)) return;
    const records = [removeRecord(agent.id)];
    const remaining = projectPending(visibleAgents(), records, { keyOf: agentIdOf });
    openConversation(selectAgentId(remaining, null));
    await runOptimistic({
      scope: pendingAgentsScope(),
      records,
      call: () => call("agent.remove", { entity_id: entityId, agent_id: agent.id }),
      failureSummary: "Could not remove the agent",
      onRevert: () => {
        openConversation(agent.id);
        paint();
      },
    });
    await refreshFeed();
    await refresh();
  };

  // ---- TUI ------------------------------------------------------------------

  /// The same panel, attached to the agent's own screen.
  ///
  /// The PTY is sized to the panel, not to a full-width tab: the pane's own
  /// observer measures the box it is mounted in and resizes the terminal (and
  /// the PTY behind it) to match, so a TUI drawn for 120 columns redraws for
  /// the rail's width. The touch key bar comes with the pane.
  const mountTui = () => {
    const body = host.querySelector("#rail-body");
    if (!body) return;
    body.classList.add("rail-body-tui");
    const agent = agentOf(selectedId);
    const controller = controllerForAgent(agent);
    const target = entity.entityId && !entity.adoptable
      ? { id: controller?.identity.entityId || entity.entityId, ...(controller ? { agent_id: controller.identity.agentId } : {}) }
      : { project_id: entity.projectId, ...(entity.worktreeId ? { worktree_id: entity.worktreeId } : {}) };
    tui = mountAgentTab(body, target, {
      idleLabel: "No agent session is running here",
      onStart: startAgent,
    });
  };

  /** Put the open agent back on its screen. It names no harness: the agent is
   *  locked to the one it was created on, and its conversation is waiting
   *  there.
   *
   *  The daemon answers the start before the harness exists, so the row wears
   *  AGENT_STARTING from the press: the answer to a start is the entity's own
   *  next word about the session, never this reply. A start that outlives the
   *  timer settles the same way — the patch stands, and nothing is thrown at
   *  the pane that would paint a refusal over a session coming up. */
  const startAgent = async () => {
    const agent = agentOf(selectedId);
    const call = chatRepository.currentCall();
    let started = null;
    let refusal = null;
    const settled = await runOptimistic({
      scope: pendingAgentsScope(),
      records: agent ? [startingRecord(agent.id)] : [],
      call: async () => {
        const entityId = await ensureEntity(call);
        started = await replyOrNothing(
          call("agent.start", {
            id: entityId,
            ...(agent ? { agent_id: agent.id } : {}),
          }),
        );
      },
      notify: false,
      onRevert: (error) => {
        refusal = error;
      },
    });
    if (started && started.agent_id) {
      selectedId = started.agent_id;
      railView.chooseAgent(selectedId);
    }
    await refresh();
    if (!settled) throw refusal;
    return started;
  };

  const disposeTui = () => {
    if (!tui) return;
    tui.dispose();
    tui = null;
  };

  // ---- lifecycle ------------------------------------------------------------

  paint();
  // The feed already names this work item — agents included — and the cached
  // snapshot replays synchronously at subscribe. Standing the strip and panel
  // up from it means a branch switch shows the conversation surface, with the
  // seeded history, before the first live read answers; the read reconciles.
  // Chat rendering last, after a full round trip, was the reviewer's headline
  // complaint — this is what removes the round trip from the first paint.
  const feedSeedEntity = feedRow ? railEntity(feedRow, context.kind) : null;
  if (feedSeedEntity && feedSeedEntity.agents.length && !visibleAgents().length) {
    reconcileOptimistic(pendingAgentsScope(), feedSeedEntity.agents, { keyOf: agentIdOf });
    entity = feedSeedEntity;
    // The same selection the live path makes, so the seed and the read agree
    // on whose conversation the panel is showing. Only a row that names its
    // agents seeds: an agentless row has no selection to make, and making one
    // anyway would wipe the remembered choice the live read is about to honor.
    chooseAgent(selectAgentId(visibleAgents(), selectedId));
    paint();
  }
  refresh();
  readProjectAgent();
  // The harnesses and their models, asked of the machine this rail is mounted
  // on and held there (core/modelCatalog.js). The new-agent view leads with
  // that bridge's default, which is this answer's to give, so a paint that
  // lands before it holds the client's own first harness and moves when the
  // answer does.
  Promise.all([deviceCatalog(context.deviceId), readProjectAgentSetting()]).then(([offered, setting]) => {
    if (disposed) return;
    catalog = offered;
    projectAgentSetting = setting;
    seedNewAgentDefaults();
    paint();
  });
  // Read at delivery, not here: the rail learns which entity it is standing on
  // from its first answer, and a branch that has to be adopted has no entity id
  // at all until something mutates it.
  poll = watchChanges({
    refresh,
    intervalMs: RAIL_POLL_MS,
    entity: () => [entity.entityId, entity.worktreeId, alongsideEntity.entityId],
    // Focus tier: the rail paints lifecycle and conversation, so those are the
    // kinds it asks the bridge for.
    kinds: ["state", "thread"],
    mode: "realtime",
  });
  // The elapsed-time clock ticks between feed reads, same as the toolbar's
  // used to.
  statusTicker = setInterval(paintRailStatus, 1000);
  document.addEventListener("keydown", dismissOnEscape);
  document.addEventListener("pointerdown", dismissOnOutsidePointer);
  window.addEventListener("hashchange", dismissPopover);
  const cancelPanelMotion = () => panelMotion.cancel();
  panelMotion = createChatPanelMotion(host, { onPhase: syncPopover });
  window.addEventListener("resize", cancelPanelMotion);

  return {
    dispose() {
      disposed = true;
      panelMotion.cancel();
      if (poll) poll.dispose();
      poll = null;
      clearInterval(statusTicker);
      statusTicker = null;
      unsubscribePending();
      unsubscribeFeed();
      disposeTitleMotion();
      disposeTui();
      disposeSurfaces();
      completionToast.dispose();
      completionTracker.reset();
      disposeComposerClearance?.();
      disposeComposerClearance = null;
      closeSurfaceMenu?.();
      composerControl?.dispose?.();
      unsubscribeComposerController?.();
      unsubscribeComposerController = null;
      releaseFaces();
      document.removeEventListener("keydown", dismissOnEscape);
      document.removeEventListener("pointerdown", dismissOnOutsidePointer);
      window.removeEventListener("hashchange", dismissPopover);
      window.removeEventListener("resize", cancelPanelMotion);
      if (ownsChatRepository) chatRepository.dispose();
      host.classList.remove(POPOVER_CLASS, COLLAPSED_CLASS, "rail-unpinned");
      host.innerHTML = "";
    },
  };
}
