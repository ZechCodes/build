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

import { App, go, loadModelCatalog } from "../app.js";
import { createPatternRenderer } from "./agentCanvas.js";
import { hashString } from "./patternMotion.js";
import { watchChanges } from "./changeEvents.js";
import { createAdoptingCall, createPrimaryAdoptingCall } from "./adoption.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import {
  AGENT_STARTING,
  QUIET_SHAPE,
  agentCanInterrupt,
  agentHasTerminal,
  agentIsUp,
  agentSessionAnswered,
  agentStartFailure,
  agentHeading,
  agentTitle,
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
import { confirmAction } from "./confirm.js";
import {
  insertRecord,
  isProvisionalKey,
  patchRecord,
  projectPending,
  removeRecord,
} from "./optimistic.js";
import { EXITING_ATTRIBUTE, patchList, rekeyEntry } from "./patchList.js";
import { hide, motionSettled, reveal } from "./motion.js";
import { composerHtml, mountComposerModelMenu } from "./composer.js";
import { catalogForProvider, creatableCatalog, effortLevels, effortSupported, matchCatalogModel, modelParams } from "./modelPicker.js";
import { markSeen } from "./inboxView.js";
import { notifyError } from "./notify.js";
import { currentCacheScope } from "./cacheScope.js";
import { createConversationCache } from "./conversationCache.js";
import { createChatRepository } from "./chatRepository.js";
import { createAgentRailContext } from "./agentRailContext.js";
import { entityIdOf } from "./entityId.js";
import { replyOrNothing } from "./session.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  MUTATION_THREAD_PAGE,
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
import { surfaceMenuOptions, surfacesAfterGrace } from "./agentSurfacesModel.js";
import { menuButtonMarkup, mountMenuIfChanged } from "./splitButton.js";
import { mountAgentTab } from "./surfaceTabs.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { providerInSameFamily } from "./providerCatalog.js";
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

const EXPANDED_KEY = "build.rail.expanded";
const COMPOSER_IDS = { input: "railinput", send: "railsend", hint: "railhint" };
const RAIL_STATUS_ID = "rail-status";
const RAIL_STATUS_LEAD_ID = "rail-status-lead";
const RAIL_STATUS_PILLS_ID = "rail-status-pills";
const RAIL_STATUS_GIT_ID = "rail-status-git";
const RAIL_VIEWER_ID = "rail-surfaces-viewer";
const WORKING_WORD_SELECTOR = ".rail-status-working-word";
const STATUS_TEXT_SELECTOR = ".rail-status-text";
const STANDING_PILL_SELECTOR = `.surface-pill:not([${EXITING_ATTRIBUTE}])`;
const SURFACE_MENU_CLASS = "rail-surface-menu";
const SURFACE_MENU_SELECTOR = `.${SURFACE_MENU_CLASS}`;
const SURFACE_MENU_LABEL = "⋯";
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

function railChatDependencies(context) {
  const cacheScope = context.cacheScope || App.cacheScope || currentCacheScope();
  const injectedRepository = context.chatRepository || App.chatRepository;
  return {
    cacheScope,
    ownsRepository: !injectedRepository,
    repository: injectedRepository || createChatRepository({
      scope: cacheScope || {},
      viewingContext: context.viewingContext || App.viewingContext,
      // Standalone compatibility only. Application mounts inject a scoped
      // repository which connection lifecycle retargets explicitly.
      call: (method, params) => App.call(method, params),
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

const readExpanded = () => {
  try {
    return localStorage.getItem(EXPANDED_KEY) !== "0";
  } catch {
    return true;
  }
};

const writeExpanded = (on) => {
  try {
    localStorage.setItem(EXPANDED_KEY, on ? "1" : "0");
  } catch {
    /* private mode: the choice lasts the session */
  }
};

export function bubbleHtml(bubble) {
  const classes = ["rail-bubble", `rail-bubble-${bubble.type}`];
  if (bubble.active) classes.push("active");
  if (bubble.working) classes.push("working");
  if (bubble.starting) classes.push("starting");
  // A pattern IS the bubble's face, so it takes the label's place: a canvas
  // for core/agentCanvas.js to paint into, named by the ordinal it wears.
  // The `+` and anything else that speaks in a glyph keeps a label.
  const count = bubble.unread
    ? `<span class="rail-count">${esc(String(bubble.unread))}</span>`
    : `<span class="rail-count" hidden></span>`;
  const face = bubble.pattern
    ? `<canvas class="rail-glyph" aria-hidden="true"></canvas>${count}`
    : `<span class="rail-bubble-label">${esc(bubble.label)}</span>`;
  const pattern = bubble.pattern ? ` data-pattern="${esc(String(bubble.pattern))}"` : "";
  return `<button type="button" class="${classes.join(" ")}" data-bubble="${esc(bubble.type)}"
    data-agent="${esc(bubble.id)}"${pattern} title="${esc(bubble.title)}"
    aria-label="${esc(bubble.title)}">${face}</button>`;
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

function surfaceMenuHtml(options) {
  return options.length ? menuButtonMarkup(SURFACE_MENU_LABEL, options, { title: SURFACE_MENU_TITLE, icon: true }) : "";
}

function surfaceMenuRegionHtml(options) {
  return `<span class="${SURFACE_MENU_CLASS}">${surfaceMenuHtml(options)}</span>`;
}

/** The button that takes this agent off the branch, or nothing when it cannot be. */
function railRemoveButtonHtml(who, removable) {
  if (!removable) return "";
  const removeTitle = `Remove ${who} from this branch`;
  return `<button type="button" class="iconbtn rail-remove" title="${esc(removeTitle)}"
        aria-label="${esc(removeTitle)}">−</button>`;
}

/** The TUI toggle, or nothing for an agent with no basement to show. */
function railTuiButtonHtml(mode, hasTerminal) {
  if (!hasTerminal) return "";
  const showingTui = mode === "tui";
  const tuiTitle = showingTui ? "Back to the conversation" : "Show the terminal";
  return `<button type="button" class="rail-mode rail-tui${showingTui ? " on" : ""}"
        aria-pressed="${showingTui}" title="${tuiTitle}">TUI</button>`;
}

export function panelHeadHtml(who, mode, { provider = "", removable = false, hasTerminal = true, surfaceOptions = [], heading = null } = {}) {
  return `<div class="rail-head">
    ${harnessIconHtml(provider)}
    ${railWhoHtml(who, heading)}
    ${railTuiButtonHtml(mode, hasTerminal)}
    ${surfaceMenuRegionHtml(surfaceOptions)}
    ${railRemoveButtonHtml(who, removable)}<button type="button" class="iconbtn rail-collapse" title="Collapse the conversation"
      aria-label="Collapse the conversation">›</button>
  </div>`;
}

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
 */
export function mountAgentRail(host, context) {
  if (!host) return { dispose() {} };
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
  let selectedId = railView.selectedAgentId();
  selection.set(selectedId);
  // A collapsed rail has no composer to focus at all — the human just cut
  // this branch and is about to type into it, so that intent outranks
  // whatever they left the rail at on the last one.
  let expanded = context.autofocusComposer === true || readExpanded();
  let mode = railView.panelMode();
  let poll = null;
  let disposed = false;
  let tui = null; // the mounted PTY pane, in TUI mode
  const transientThreadCache = createThreadCache();
  let threadCache = transientThreadCache;
  // Choosing the next agent's harness, with the chooser in the panel. Entered
  // by the strip's `+`, left by the send that creates the agent or by opening
  // any existing bubble.
  let addingAgent = false;
  let threadAgentId = null; // whose conversation the cache holds
  let loadingOlderItems = false; // a page of history is in flight
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
    return addressedCacheIdentity({
      cacheScope,
      context,
      feedRow,
      selectedId,
      controller: controllerForAgent(agentOf(selectedId)),
    });
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
      seededSurfaces = { surfaces: surfacesAfterGrace(seen.surfaces, seen.at, Date.now()), at: seen.at };
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
    bindConversationCache().absorbSurfaces(agent ? agent.surfaces : null);
  };

  const resetConversationCache = () => {
    conversationCache = null;
    absorbedThreadPayload = null;
    seededSurfaces = null;
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
  let composerModelMenu = null;
  let composerController = null;
  let unsubscribeComposerController = null;
  let surfacesBlock = null;
  let surfaceOverlay = null; // the surface a menu option opened, over the panel
  let closeSurfaceMenu = null; // shuts the head's ⋯, and with it its outside-press watch


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

  const seedNewAgentDefaults = () => {
    if (!catalog || provisionalController().choice().provider) return;
    writeNewAgentChoice(clampStoredAgentChoice(catalog, loadAgentDefaults()));
  };

  /** That choice as `agent.add` params: empties omitted, so the harness's own
   *  default stands where nothing was said. */
  const newAgentParams = (choice = newAgentChoice()) => {
    const { models } = catalogForProvider(catalog || {}, choice.provider);
    return modelParams(models || [], choice.model || choice.requestedModel, choice.effort, choice.provider);
  };

  /** The adopting caller for a checkout Build owns nothing in.
   *
   *  A view whose other surfaces can adopt too owns the adopter and hands it
   *  down (`context.adopting`), so the rail and the Changes review claim the
   *  checkout once between them. Standing alone, the rail makes its own once
   *  the payload says which checkout it is, and keeps it — it holds the run it
   *  mints. */
  const adoptingCall = (call) => {
    if (context.adopting) return context.adopting() || null;
    if (!adopting && entity.adoptable && entity.projectId) {
      adopting = entity.primary
        ? createPrimaryAdoptingCall(call, entity.projectId)
        : createAdoptingCall(call, entity.projectId, entity.worktreeId);
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
    feedRow = toolbarIdentity(feedRoute(), { items: feed.items || [], projects: feed.projects || [] }).row;
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

  const refresh = async () => {
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
  };

  // ---- painting -------------------------------------------------------------

  const releaseFaces = () => {
    faces.forEach((face) => face.renderer.destroy());
    faces.clear();
  };

  const wireBubble = (button, bubble) => {
    button.onclick = () => pressBubble(button.dataset.bubble, button.dataset.agent);
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
      agents: visibleAgents(), selectedId, kind: entity.kind, chatCapable: entity.chatCapable !== false,
      addingAgent,
    }));
    let panel = host.querySelector("#rail-panel");
    if (expanded && !panel) {
      panel = document.createElement("div");
      panel.className = "rail-panel";
      panel.id = "rail-panel";
      host.insertBefore(panel, strip);
    } else if (!expanded && panel) {
      disposeTui();
      disposeSurfaces();
      closeSurfaceMenu?.();
      panel.remove();
    }
    if (expanded) paintPanel();
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
    const who = agent ? agentTitle(agent) : entity.kind === "issue" ? "Issue agent" : "New agent";
    // The head wears the topic the agent named its work with, and "Starting"
    // until it has; the harness name stays as the hover title and the remove
    // button's wording. The harness icon beside it says which harness.
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
    // The head is rewritten only when what it SAYS changed: the name, whether
    // this agent can be taken back off, and whether it has a basement.
    const wantedHead = `${who}:${provider}:${heading.text}:${removable ? "removable" : "kept"}:${hasTerminal ? "tui" : "chatonly"}`;
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = wantedPanelBody();
    if (panel.dataset.body !== wantedBody) {
      disposeTui();
      disposeSurfaces();
      closeSurfaceMenu?.();
      panel.innerHTML = `${panelHeadHtml(who, shownMode, { provider, removable, hasTerminal, surfaceOptions: surfaceMenuOptionsInFocus(), heading })}
        <div class="rail-body" id="rail-body"></div>
        ${shownMode === "chat"
          ? `${rememberedConversationIsLoading() || entity.chatCapable === false ? "" : composerRowHtml()}`
          : ""}`;
      panel.dataset.head = wantedHead;
      panel.dataset.body = wantedBody;
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
        if (autofocusComposerPending) {
          autofocusComposerPending = false;
          panel.querySelector(`#${COMPOSER_IDS.input}`)?.focus();
        }
      }
    } else if (panel.dataset.head !== wantedHead) {
      // The name changed under the panel (an agent whose provider was picked
      // after the fact, or one that just named its topic), or the last agent
      // beside this one went away. Nothing
      // else in the head can move on a poll, and rewriting it every tick would
      // eat a press that landed mid-repaint.
      closeSurfaceMenu?.();
      panel.querySelector(".rail-head").outerHTML = panelHeadHtml(who, shownMode, {
        provider,
        removable,
        hasTerminal,
        surfaceOptions: surfaceMenuOptionsInFocus(),
        heading,
      });
      panel.dataset.head = wantedHead;
      wireHead(panel);
    }
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
    if (remove) remove.onclick = () => removeAgent();
    const collapse = panel.querySelector(".rail-collapse");
    if (collapse) {
      collapse.onclick = () => {
        expanded = false;
        writeExpanded(false);
        disposeTui();
        paint();
      };
    }
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
    if (threadCache.absorbOlderPage(page, request.seek)) paintChat({ olderItemsPrepended: true });
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
      // chosen under the harness beside it.
      writeNewAgentChoice(
        reconcileAgentChoice({ ...newAgentChoice(), provider: card.dataset.provider }, { providerChanged: true }),
      );
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
    paintTimeline(body, threadFor(), olderItemsPrepended);
    // Assignment rather than a listener: the scroller outlives every repaint,
    // and adding one per paint would ask for the same page once per tick.
    body.onscroll = () => {
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
    recoveryHost.innerHTML = chatRecoveryHtml(controller);
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
    wireThreadLinks(body, openLink);
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
    if (live) return { surfaces: live, at: Date.now() };
    if (agent && seededSurfaces) return seededSurfaces;
    return { surfaces: null, at: Date.now() };
  };

  const surfaceMenuOptionsInFocus = () => surfaceMenuOptions(surfacesSeen().surfaces);

  const mountSurfaces = (panel) => {
    const pillHost = panel.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const viewerHost = panel.querySelector(`#${RAIL_VIEWER_ID}`);
    if (!pillHost || !viewerHost) return;
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
    if (!surfacesBlock) return;
    surfacesBlock.dispose();
    surfacesBlock = null;
  };

  const syncSurfaces = () => {
    if (!surfacesBlock) return;
    const seen = surfacesSeen();
    surfacesBlock.set(seen.surfaces, seen.at);
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

  /** A reference in the conversation goes where it points, as far as the two
   *  work-item surfaces can take it. */
  const openLink = (link) => {
    if (link.issue_id || link.plan_id) {
      go({ name: "issue", projectId: entity.projectId, id: link.issue_id || link.plan_id });
      return;
    }
    if (link.kind === "file" && entity.kind === "workspace") {
      go({
        name: "workspace",
        projectId: entity.projectId,
        workspaceId: context.workspaceId,
        sourceId: context.sourceId,
        tab: "files",
        ...(link.path ? { file: link.path, line: link.line } : {}),
      });
      return;
    }
    if (link.kind === "file" && entity.kind === "branch" && entity.branch) {
      go({ name: "branch", projectId: entity.projectId, branch: entity.branch, tab: "files" });
    }
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
    },
  });

  const rekeyPostedMessage = (handle, messageKey, provisional, posted) => {
    const sequence = (posted && posted.posted_sequence) ?? null;
    if (sequence === null) handle.drop(messageKey);
    else handle.rekey(messageKey, String(sequence), { ...provisional, data: { ...provisional.data, sequence } });
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
    if (posted) rekeyPostedMessage(handle, messageKey, provisionalMessage, posted);
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

  const pressBubble = (type, agentId) => {
    if (type === "add") {
      pressAddBubble();
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
    // The bubble already open is the way back out: press it again to collapse.
    expanded = !expanded;
    writeExpanded(expanded);
    if (!expanded) disposeTui();
    paint();
  };

  /** Open this agent's conversation in the panel, with the panel out. */
  const openAgent = (agentId) => {
    addingAgent = false; // opening a real conversation ends the chooser
    openConversation(agentId);
    expanded = true;
    writeExpanded(true);
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
      expanded = true;
      writeExpanded(true);
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
  const removeAgent = async () => {
    const agent = settledAgentInFocus();
    if (!agent || !entity.entityId) return;
    const call = chatRepository.currentCall();
    if (!(await confirmAction(removeAgentConfirm(agent, entity.kind)))) return;
    if (isPending(pendingAgentsScope(), agent.id)) return;
    const records = [removeRecord(agent.id)];
    const remaining = projectPending(visibleAgents(), records, { keyOf: agentIdOf });
    openConversation(selectAgentId(remaining, null));
    await runOptimistic({
      scope: pendingAgentsScope(),
      records,
      call: () => call("agent.remove", { entity_id: entity.entityId, agent_id: agent.id }),
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
  // The harnesses and their models, fetched once per session (app.js caches it).
  // The new-agent view leads with the account's default, which is this answer's
  // to give, so a paint that lands before it holds the client's own first
  // harness and moves when the answer does.
  loadModelCatalog().then((answer) => {
    if (disposed) return;
    catalog = answer;
    seedNewAgentDefaults();
    paint();
  });
  // Read at delivery, not here: the rail learns which entity it is standing on
  // from its first answer, and a branch that has to be adopted has no entity id
  // at all until something mutates it.
  poll = watchChanges({
    refresh,
    intervalMs: RAIL_POLL_MS,
    entity: () => [entity.entityId, entity.worktreeId],
  });
  // The elapsed-time clock ticks between feed reads, same as the toolbar's
  // used to.
  statusTicker = setInterval(paintRailStatus, 1000);

  return {
    dispose() {
      disposed = true;
      if (poll) poll.dispose();
      poll = null;
      clearInterval(statusTicker);
      statusTicker = null;
      unsubscribePending();
      unsubscribeFeed();
      disposeTui();
      disposeSurfaces();
      closeSurfaceMenu?.();
      composerControl?.dispose?.();
      unsubscribeComposerController?.();
      unsubscribeComposerController = null;
      releaseFaces();
      if (ownsChatRepository) chatRepository.dispose();
      host.innerHTML = "";
    },
  };
}
