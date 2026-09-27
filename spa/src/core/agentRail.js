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
// One work item, one rail: `mountAgentRail` is given the route, resolves it
// against this device's cached rows (core/railWorkItem.js), and every agent it
// renders comes off that row's agents[]. A saved conversation is on the first
// frame out of the cache; a cold one asks the shared window syncer immediately
// and then moves only when that cache record moves.
//
// With one exception, and it is the project's agent. A project's agent is
// reachable from every workspace in the project — that is what makes it the
// project's — so a workspace's rail carries it as one bubble above a line, and
// pressing it stands this same rail on the project's conversation without the
// page leaving the workspace. The rail is on one of the two at a time and reads
// the other beside it, so the strip says what both are doing.

import { setAttr, setData } from "../dom.js";
import { AGENT_CAPABILITIES, AGENT_ROLES, modelForRole } from "./agentRoles.js";
import { wireReaderMotion } from "./paintKeepingPlace.js";
import { chatOverlaysPage, panelDocksByDefault } from "./railLayout.js";
import { App, go } from "../app.js";
import { createPatternRenderer } from "./agentCanvas.js";
import { hashString } from "./patternMotion.js";
import { createAdoptingCall } from "./adoption.js";
import { agentDefaultsForWorkspace, agentDefaultsInWorkspace } from "./workspaceDefaults.js";
import { projectAgentChoiceOf } from "./projectAgentSetting.js";
import { deviceKey, workspaceKey } from "./deviceKey.js";
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
  rememberedAgentId,
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
import { mountUsageLimitBanner } from "./usageLimits.js";
import { createTimelineSlice } from "./timelineSlice.js";
import { hide, motionSettled, reveal, setMotionRowHtml } from "./motion.js";
import { composerGaugeHtml, composerHtml, mountComposerModelMenu } from "./composer.js";
import { mountContextGauge } from "./contextGauge.js";
import { catalogForProvider, creatableCatalog, effortLevels, effortSupported, matchCatalogModel, modelParams } from "./modelPicker.js";
import { markSeen } from "./inboxView.js";
import { onReaderReturns, readerIsHere } from "./readerPresence.js";
import { notifyError } from "./notify.js";
import { deviceFeedView } from "./deviceContexts.js";
import { deviceCatalog, followDeviceCatalog } from "./inboxDevices.js";
import { createConversationCache, withdrawProvisionalMessage, writeProvisionalMessage } from "./conversationCache.js";
import { syncThreadWindow } from "./threadSync.js";
import {
  conversationRecordAddress,
  postSubmission,
  provisionalMessageEntry,
  rekeyPostedMessage,
  settleProvisional,
} from "./threadSend.js";
import { createChatRepository } from "./chatRepository.js";
import { createAgentRailContext } from "./agentRailContext.js";
import { hashFromRoute } from "./router.js";
import { createRailWorkItem } from "./railWorkItem.js";
import { forgetRevisionBodies, revisionContents } from "./revisionBodies.js";
import { readCached, subscribeCache } from "./localCache.js";
import { scopeFor } from "./cacheScope.js";
import { replyOrNothing } from "./session.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { fetchThreadAttachment } from "./threadAttachmentPages.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { agentLabels as agentLabelsOf, workspaceAgents } from "./trackerAssignee.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  activityRunDrawn,
  activityRunThroughAt,
  chatPaintFingerprint,
  createThreadCache,
  cutRunKeyAt,
  digestsOf,
  paintThreadEntries,
  paintThreadKeepingPlace,
  jumpToUserMessage,
  scheduleUserMessageTickSync,
  syncUserMessageTicks,
  readThroughSequence,
  pressedActivityRunKey,
  revealThreadSequence,
  threadOfferState,
  timelineEntries,
  wireThreadAttachments,
  wireThreadOptions,
  wireThreadComposer,
  threadItemKey,
  pressIsTheBrowsers,
  wireThreadLinks,
  wireThreadRevisionLinks,
  wireThreadSentMessages,
  wireThreadArrivals,
} from "./thread.js";
import { createActivityRuns } from "./activityRuns.js";
import { createUnreadMarker } from "./unreadAnchor.js";
import { wireExpansionReveal } from "./revealExpanded.js";
import { digestCovering, runDigestToFetch } from "./activityDigest.js";
import { timedPaint } from "./paintTiming.js";
import { mountAgentSurfaces, openSurfaceOverlay } from "./agentSurfaces.js";
import { mountAgentIssues } from "./trackerAgentIssuesEntry.js";
import { referenceLinks } from "./referenceTargets.js";
import { ISSUES_ENTRY_KIND } from "./agentSurfacesModel.js";
import { mountAgentObservation } from "./agentObservation.js";
import { createTaskCompletionTracker } from "./taskCompletionModel.js";
import { mountTaskCompletionToast } from "./taskCompletionToast.js";
import { surfaceMenuGroup, surfacesAfterGrace } from "./agentSurfacesModel.js";
import {
  detailLevelMenuGroup,
  detailLevelOfOptionId,
  itemsAtDetailLevel,
  defaultDetailLevel,
  readThroughHiddenItems,
} from "./conversationDetail.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { surfaceSessionGeneration } from "./surfacesCache.js";
import { groupedMenuButtonMarkup, mountMenuIfChanged } from "./splitButton.js";
import { mountAgentTab } from "./surfaceTabs.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { PIN_CLASS, pinButtonHtml, syncPinButton } from "./pinControl.js";
import { WATCH_BUTTON_SELECTOR, createWatchToggle, syncWatchButton, watchButtonHtml } from "./watchToggle.js";
import { carriesWatching } from "./trackerWatch.js";
import {
  carriesCompactionSettings,
  compactionLimitOfOptionId,
  compactionMenuGroup,
  createCompactionChoice,
} from "./conversationCompaction.js";
import { providerInSameFamily } from "./providerCatalog.js";
import { mountComposerClearance } from "./composerClearance.js";
import { createChatPanelMotion } from "./chatPanelMotion.js";
import { createChatTitleMotion } from "./chatTitleMotion.js";
import { createAgentOverview, overviewHtml } from "./agentOverview.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { ICON_CHAT_OVERVIEW } from "./icons.js";
import { scopeLinkHtml } from "./scopeLink.js";
import "../styles/shell.css";
import { fieldTraits } from "./fieldTraits.js";

/** How close to the top of the conversation counts as asking for the page
 *  above it. Not zero: a reader flicking upwards should have the history on
 *  its way before they land, so the join is one they scroll through rather
 *  than wait at. */
const OLDER_ITEMS_TRIGGER_PX = 120;

/** Where the reader's pin choice is kept. Named for what the flag used to mean
 *  — the panel out or shut — and holding what it means now: docked beside the
 *  work, or a popover on the bubble strip. One key, so a reader who had the
 *  panel out keeps a pinned panel. */
/** The thing this rail's pin docks, as the reader would name it. */
const PANEL_SUBJECT = "conversation";
const POPOVER_CLASS = "rail-popover";
const COLLAPSED_CLASS = "rail-collapsed";
const ANCHOR_PROPERTY = "--rail-anchor";
/** Every control on the strip that selects something: the bubbles, the `+`
 *  and the chat overview's — not the line under the project's agent. */
const STRIP_BUTTONS = ".rail-strip > [data-bubble]";
/** The panel while it shows the chat overview rather than a conversation. */
const OVERVIEW_BODY = "overview";
const OVERVIEW_PANEL = `#rail-panel[data-body="${OVERVIEW_BODY}"]`;
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
const SURFACE_MENU_TITLE = "Conversation menu";
const AGENT_NOT_YET_BORN = "ghost";
const COMPACTION_REFUSED = "Build could not change when this chat compacts.";

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
  // Every read this rail makes is addressed through the scope of the machine it
  // is mounted on. A view that owns one hands it down; standing alone, the rail
  // takes the same object the rest of that device's surfaces hold.
  const cacheScope = context.cacheScope || scopeFor(context.deviceId);
  const injectedRepository = context.chatRepository;
  return {
    cacheScope,
    ownsRepository: !injectedRepository,
    repository: injectedRepository || createChatRepository({
      scope: cacheScope || {},
      viewingContext: context.viewingContext || App.viewingContext,
      call: (...args) => context.call(...args),
    }),
  };
}

/** The store the composer's tray paints from while this rail stands on the
 *  project's conversation reached from a workspace: the workspace leads the
 *  chips the reader collected, and cannot be taken off.
 *
 *  A wrapper rather than a write into the store: nothing else on screen is
 *  standing in that workspace, and the store is the whole app's. The tray hands
 *  back the indices it rendered, so the stamp's place comes off them on the way
 *  down.
 */
function stampedViewingContext(store, stamp) {
  if (!store || !stamp) return store;
  const wrapped = {
    ...store,
    snapshot() {
      const held = store.snapshot?.();
      // Nothing is sent to a bridge that takes no context, so nothing is shown.
      if (store.isEnabled?.() === false) return held;
      return { version: 1, items: [stamp, ...(held?.items || [])] };
    },
    remove: (index) => store.remove?.(index - 1),
    removeMany: (indices) => store.removeMany?.(indices.map((index) => index - 1)),
    // The store announces its own snapshot; this tray paints from ours.
    subscribe: (listener) => store.subscribe?.(() => listener(wrapped.snapshot())),
  };
  return wrapped;
}

/** Where this rail is standing, when that is a workspace it reached the
 *  project's conversation from: what every message sent from here wears, and
 *  the store the composer's tray paints it from. Null and the store itself for
 *  every other rail. */
const railStandingPlace = (context) => {
  const fromWorkspace = context.fromWorkspace || null;
  const store = context.viewingContext || App.viewingContext;
  return { fromWorkspace, composerViewingContext: stampedViewingContext(store, fromWorkspace) };
};

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

/** The other record kind the rail reads itself, beside the work item
 *  core/railWorkItem.js resolves: the machine's project list. Both are the
 *  sync layer's to write. */
const PROJECTS_RECORD_KIND = "projects";

/** One conversation record, named, so a watcher can tell whether the panel has
 *  moved to another one. */
const threadAddressKey = (address) =>
  address ? [address.deviceId, address.entityId, address.sub].join("|") : "";

const fallbackCacheEntityId = (context, railEntityId) => {
  if (context.kind === "issue") return context.issueId;
  return railEntityId || null;
};
const cacheEntityId = (identity, context, railEntityId) => identity?.entityId || fallbackCacheEntityId(context, railEntityId);
const cacheAgentId = (identity, selectedId) => identity?.agentId || selectedId || "";
const cacheConversationId = (identity, selectedId) => identity?.conversationId || selectedId || "";

function addressedCacheIdentity({ cacheScope, context, railEntityId, selectedId, controller }) {
  if (!cacheScope) return null;
  const identity = controller?.identity;
  const entityId = cacheEntityId(identity, context, railEntityId);
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
  // Chat/view memory belongs to the injected application repository and is
  // retired with its account/device scope. What is left here is the session's
  // memo of the bodies a conversation points at.
  forgetRevisionBodies();
}



/** Whether the panel is docked before anyone has said. Beside the work it is:
 *  the conversation and the work are both on screen and neither costs the other
 *  anything. Laid over the work it is not, or a workspace opens showing its
 *  conversation and nothing else — no Files, no Changes, and nothing on screen
 *  saying the strip is the way back to them. A reader who has made the choice
 *  keeps it, at either width. */
const PINNED_ADDRESS = uiAddress({ view: "agent-rail", kind: "fold", sub: "pinned" });

/** Which way the popover faces, which is which way the strip runs: down the
 *  view's right edge on a desktop, across its foot on a phone. */
const stripRunsAcross = () => chatOverlaysPage();

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
  // The project's own agent, on the project's own page: squared off and
  // wearing the initial, the way it is above the line on a workspace's strip.
  if (bubble.project && bubble.type !== "project") classes.push("rail-bubble-project");
  if (bubble.active) classes.push("active");
  if (bubble.working) classes.push("working");
  if (bubble.starting) classes.push("starting");
  // An agent the reader does not watch, on the strip only while it is open (#105).
  if (bubble.unwatched) classes.push("rail-bubble-unwatched");
  return classes.join(" ");
};

export function bubbleHtml(bubble) {
  // The line between the project's agent and this work item's own. It says
  // nothing and is pressed by nobody, so it is not a button.
  if (bubble.type === "separator") return `<div class="rail-sep" role="separator"></div>`;
  // The chat overview's control: a bare icon right after the `+` (#117), but
  // selected, opened and pointed at exactly as a bubble is (#148).
  // Each says what it opens, and whether it has, in its markup: the strip is
  // repainted on every push, and markup that left these to syncPopover had them
  // stripped and written back each time.
  if (bubble.type === "overview") return `<button type="button" class="rail-overview-toggle${bubble.active ? " active" : ""}" data-bubble="overview"
    title="${esc(bubble.title)}" aria-label="${esc(bubble.title)}"
    aria-expanded="${Boolean(bubble.expanded)}" aria-controls="rail-panel">${ICON_CHAT_OVERVIEW}</button>`;
  const pattern = bubble.pattern ? ` data-pattern="${esc(String(bubble.pattern))}"` : "";
  return `<button type="button" class="${bubbleClasses(bubble)}" data-bubble="${esc(bubble.type)}"
    data-agent="${esc(bubble.id)}"${pattern} title="${esc(bubble.title)}"
    aria-label="${esc(bubble.title)}" aria-expanded="${Boolean(bubble.expanded)}"
    aria-controls="rail-panel">${bubbleFaceHtml(bubble)}</button>`;
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
  const words = status.working || status.starting || "";
  if (text && text.textContent !== words) text.textContent = words;
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

/** The conversation's ⋮: a menu in sections (core/splitButton.js), or nothing
 *  for a head with no sections to offer. */
function surfaceMenuHtml(groups) {
  return groups.length ? groupedMenuButtonMarkup(SURFACE_MENU_LABEL, groups, { title: SURFACE_MENU_TITLE, icon: true }) : "";
}

function surfaceMenuRegionHtml(groups) {
  return `<span class="${SURFACE_MENU_CLASS}">${surfaceMenuHtml(groups)}</span>`;
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

/** The project a conversation belongs to, as the way to its page.
 *
 *  It rides the head of a project's conversation opened from a work item,
 *  because that is the one place the page behind the panel is about something
 *  else and nothing else on screen says where this conversation lives. An
 *  anchor, because a project page has a URL of its own: middle-click and
 *  Cmd-click open it in a tab, exactly as they do on a reference in the
 *  conversation below. */
function railProjectChipHtml(chip) {
  if (!chip) return "";
  const label = `Go to ${chip.name}`;
  return `<a class="rail-project-chip" href="${esc(hashFromRoute(chip.route))}"
        title="${esc(label)}" aria-label="${esc(label)}">${esc(chip.name)}</a>`;
}

/** What the head is fingerprinted by where the chip is concerned: the project
 *  it names, which the rail learns after the first paint. */
const chipMark = (chip) => chip?.name || "";

/// What a head shows when the caller does not say. A spread rather than eight
/// parameter defaults: each default is a branch, and the head was over the
/// complexity cap by carrying them.
const HEAD_DEFAULTS = Object.freeze({
  provider: "",
  removable: false,
  hasTerminal: true,
  menuGroups: [],
  showHarnessIcon: true,
  heading: null,
  pinned: true,
  removalWho: "",
  projectChip: null,
  /// Null on a rail that is not offering the switch — the project's side of a
  /// swap, and any caller that predates it (#65).
  watch: null,
});

/** The watch switch, on a head that has one. */
const railWatchButtonHtml = (watch) => (watch ? watchButtonHtml(watch) : "");

export function panelHeadHtml(who, mode, given = {}) {
  const head = { ...HEAD_DEFAULTS, ...given };
  return `<div class="rail-head">
    ${head.showHarnessIcon ? harnessIconHtml(head.provider) : ""}
    ${railWhoHtml(who, head.heading)}
    ${railTuiButtonHtml(mode, head.hasTerminal)}
    ${railRemoveButtonHtml(head.removalWho || who, head.removable)}
    ${railWatchButtonHtml(head.watch)}
    ${railProjectChipHtml(head.projectChip)}
    ${pinButtonHtml({ subject: PANEL_SUBJECT, pinned: head.pinned })}
    ${surfaceMenuRegionHtml(head.menuGroups)}
  </div>`;
}

/** The workspace this rail got to the project's conversation from, as a message
 *  wears it (core/viewingContext.js), or null when the work item below is not a
 *  workspace: the id the rail was mounted with, and the name the workspace list
 *  gives it — falling back to the id, which is what the toolbar shows until
 *  that list is on disk.
 *
 *  The project's agent is reachable from every workspace in the project, so
 *  "this workspace" is a question it cannot answer unless the message says. */
const workspaceStamp = (context, name) =>
  context.kind === "workspace" && context.workspaceId
    ? { kind: "workspace", workspace_id: context.workspaceId, name: name || context.workspaceId }
    : null;

/** The project's conversation as a context of its own: this device's caller,
 *  cache and conversations, pointed at the project above the work item. Its
 *  agent is not the work item's selected agent, so the shared selection handle
 *  is deliberately left behind — and what it carries instead is the workspace
 *  it was reached from, which every message sent from here says. */
const projectAgentContext = (context, known, workspaceName, openAgentId = null) => ({
  ...context,
  kind: "project",
  projectId: context.projectAgent.projectId,
  entityId: known.entityId,
  workspaceId: null,
  fromWorkspace: workspaceStamp(context, workspaceName),
  selection: null,
  openAgentId,
  projectAgent: { ...context.projectAgent, ...known },
});

/** The work item's own context again, with the agent whose bubble asked for it
 *  open — or, where the `+` asked, the chooser up and no conversation open. */
const workItemContext = (context, known, { openAgentId = null, addingAgent = false } = {}) => ({
  ...context,
  openAgentId,
  addingAgent,
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
 * context, so it is all still there on the way back. What the repository does
 * not hold, this wrapper does: whether the panel is out, and what each side was
 * last read to be, so the rail that comes up is the one that went down.
 */
export function mountAgentRail(host, context) {
  if (!host) return { dispose() {} };
  if (!context.projectAgent?.projectId) return mountRailOnContext(host, context, null);
  // What the rail has learned about the project: its name, and the conversation
  // owner — minted by the press that opens it, never by a render. Held out here
  // so a swap does not ask for either of them again.
  let known = { entityId: context.projectAgent.entityId || null, name: context.projectAgent.name || "" };
  // The last payload read for each side, kept by the kind it answers for. The
  // rail reads both — the one it stands on and the one beside it — so a swap
  // has what the side it is standing up already is, and hands it over as that
  // mount's first answer. Without it the new rail starts from nothing and
  // paints a workspace full of agents as one that has none until its own read
  // lands.
  const payloads = new Map();
  let live = null;
  // What this machine's workspace list calls the workspace below — a message
  // sent to the project's agent from here names it, and the agent has no other
  // way of knowing where the reader was standing.
  let workspaceName = "";
  const projectSide = (openAgentId = null) => projectAgentContext(context, known, workspaceName, openAgentId);
  // Whether the panel is out belongs to the HOST, not to either conversation:
  // the swap is a re-mount, and a re-mount that read the pin again would shut
  // an unpinned card the reader had open. A press that crosses the line is a
  // press on another conversation's bubble, so it leaves the panel exactly
  // where pressing a bubble below the line leaves it — out.
  // A swap is a press on the other side's bubble, so it names what it selects;
  // the first stand opens on whatever the reader left this rail on.
  // Which kind of thing is selected — an agent or the overview — is the
  // page's too (#148): both sides remember it under the page's key, so an
  // overview opened on the project's side is the overview a fresh mount of
  // this page comes back to.
  const pageKey = createAgentRailContext(context).key;
  const stand = (standing, alongside, { panelOpen = null, selectedKind = null } = {}) => {
    live?.dispose();
    live = mountRailOnContext(host, {
      ...standing,
      // The rail can stand on the project's conversation while the route is
      // still a workspace. Overview breadth belongs to the page, not the
      // conversation selected on its strip.
      overviewPageKind: context.kind,
      pageKey,
      payload: payloads.get(standing.kind) || null,
      alongside: { ...alongside, payload: payloads.get(alongside.kind) || null },
      panelOpen,
      selectedKind,
    }, swap);
  };
  const swap = {
    learned: (facts) => {
      known = { ...known, ...facts };
    },
    read: (kind, payload) => {
      payloads.set(kind, payload);
    },
    named: (name) => {
      workspaceName = name || workspaceName;
    },
    toProject: (entityId, openAgentId = null) => {
      known = { ...known, entityId };
      stand(projectSide(openAgentId), workItemContext(context, known), { panelOpen: true, selectedKind: "agent" });
    },
    toWorkItem: (openAgentId, { adding = false } = {}) => {
      stand(workItemContext(context, known, { openAgentId, addingAgent: adding }), projectSide(),
        { panelOpen: true, selectedKind: adding ? "add" : "agent" });
    },
  };
  // The agent a URL named, where one did: the rail comes up on that
  // conversation. It may be the project's — the project's agent is reachable
  // from every workspace in the project — and the side that finds it in its own
  // half of the strip is the side that stands the rail there.
  stand(workItemContext(context, known, { openAgentId: context.openAgentId || null,
    addingAgent: context.addingAgent === true }), projectSide());
  return {
    collapse() {
      live?.collapse();
    },
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
/** The page a rail's chat overview belongs to, the scope it opens on, and
 *  where the reader's own choice of scope is kept (#117). The page is the
 *  route's, not the conversation's: standing on the project's agent from a
 *  workspace is still that workspace's page. Only a project and a workspace
 *  page move between scopes; every other page keeps the one it has. */
const pageWorkspaceId = (context, alongside) =>
  [context, alongside].find((side) => side?.kind === "workspace")?.workspaceId || null;

function overviewPageOf(context, alongside, projectId) {
  const kind = context.overviewPageKind || context.kind;
  const workspaceId = pageWorkspaceId(context, alongside);
  const moves = !!projectId && (kind === "project" || (kind === "workspace" && !!workspaceId));
  if (!moves) return { kind, workspaceId: null, scope: { kind: "page" }, address: null };
  return {
    kind,
    workspaceId,
    scope: kind === "project" ? { kind: "project" } : { kind: "workspace", workspaceId },
    address: uiAddress({ deviceId: context.deviceId, entityId: projectId, view: "agent-rail",
      kind: "overview-scope", sub: `${kind}:${workspaceId ?? ""}` }),
  };
}

/** A page with no scope to move between keeps nothing. */
const NO_SCOPE_RECORD = Object.freeze({ write: async () => {}, dispose() {} });

const isOverviewScope = (saved) => saved?.kind === "project"
  || (saved?.kind === "workspace" && typeof saved.workspaceId === "string" && saved.workspaceId !== "");

function projectAgentState(context) {
  const projectAgent = context.projectAgent || null;
  const standing = !!projectAgent && context.kind === "project";
  return {
    projectAgent,
    standing,
    entityId: (standing ? context.entityId : projectAgent?.entityId) || null,
    name: knownProjectName(context),
    projectId: railProjectId(context),
  };
}

/** The name the view already knew, on a page that has one; the project list
 *  read fills it in where the view mounted before its feed landed. */
const knownProjectName = (context) => context.projectAgent?.name || context.projectName || "";

/** The project this rail is about, on either kind of page: the one above the
 *  work item, or the one whose own page this is. Null on a rail that has
 *  neither — an issue's, say — which reads no project at all. */
function railProjectId(context) {
  if (context.projectAgent?.projectId) return context.projectAgent.projectId;
  return context.kind === "project" ? context.projectId || null : null;
}

/** Which conversation the rail was asked to keep beside the one it stands on. */
const alongsideKind = (context) => context.alongside?.kind || "project";

/** What one of a swap's two sides was last read to be, or null for a side
 *  nothing has read yet — a rail standing up for the first time. */
const seedPayload = (side) => side?.payload || null;

/** Whose conversation this mount opens on: the bubble that asked to come back
 *  here, or the one this rail was last left on — unless the reader does not
 *  watch that one, whose bubble went when they left it (#105). With a payload
 *  already in hand the choice settles the way a read settles it — the
 *  remembered conversation while it still exists, else this side's first. */
const openingAgentId = (context, railView, agents) => {
  const remembered = context.openAgentId || rememberedAgentId(agents, railView.selectedAgentId());
  return agents.length ? selectAgentId(agents, remembered) : remembered;
};

/** Whether this mount reopened a conversation from memory before this side's
 *  agents were known: the first read that names them says whether the reader
 *  still watches it (#105). */
const reopenedBeforeAgentsKnown = (context, agents, selectedId) =>
  !context.openAgentId && !agents.length && !!selectedId;

/** Which of the rail's one selection this mount opens on (#148): what a swap
 *  pressed, the `+` or the agent a URL named, else the one this page was left
 *  on — an open overview included, so a remount never trades it for a chat. */
const openingKind = (context, pageView) => {
  if (context.selectedKind) return context.selectedKind;
  if (context.addingAgent === true) return "add";
  return context.openAgentId ? "agent" : pageView.selectedKind();
};

/** Whether the panel comes up on screen.
 *
 *  An unpinned rail has no composer to focus at all — the human just cut this
 *  branch and is about to type into it, so that intent outranks whatever they
 *  left the rail at on the last one, and a URL that names a conversation asks
 *  for the same thing outright. A mount the swap stood here — one of two
 *  conversations changing places in the same panel — is handed the panel it is
 *  taking over instead: the card was already out, and a re-mount is not a
 *  reason to put it away. */
const panelStartsOut = (context, pinned) =>
  context.panelOpen ?? (pinned || context.autofocusComposer === true || !!context.openAgentId || context.addingAgent === true);

/** The verb that watches a conversation; its opposite unwatches it. */
const WATCH_VERB = "conversation.watch";

/**
 * Whether this rail offers a watch switch, and what it says to begin with.
 *
 * Null on a bridge that has never heard of watching (#65) — the verbs arrive
 * with #64, and a switch wired to one an older bridge does not know can only
 * refuse. `panelHeadHtml` reads null as "this head has no switch".
 */
function watchStateFor(context, agent) {
  if (!agent || !carriesWatching(context.deviceId)) return null;
  return { watching: agent?.watched === true, watchers: agent?.watchers || 0, pending: false };
}

/** The rail standing on ONE of its contexts. `swap` is how it moves to the
 *  other, and is null for a rail that has only one. */
function mountRailOnContext(host, context, swap) {
  // Stood up once the rail's own painters exist (below); until then a panel
  // paint has no limit to say anything about.
  let usageLimitBanner = { sync() {}, dispose() {} };
  const completionTracker = createTaskCompletionTracker();
  const completionToast = mountTaskCompletionToast(host);
  const railContext = createAgentRailContext(context);
  const key = railContext.key;
  const { cacheScope, ownsRepository: ownsChatRepository, repository: chatRepository } = railChatDependencies(context);
  const { fromWorkspace, composerViewingContext } = railStandingPlace(context);
  const {
    isPending,
    projectOptimistic,
    provisionalKey,
    reconcileOptimistic,
    runOptimistic,
    subscribeOptimistic,
  } = chatRepository.optimisticStore();
  const railView = chatRepository.railView(key);
  // Where the kind of selection is kept: the page's view, which is this rail's
  // own unless it stands on the project's side of a workspace page.
  const pageView = context.pageKey ? chatRepository.railView(context.pageKey) : railView;
  const selection = context.selection || createAgentSelection();
  // What the side this mount stands on was last read to be, where a swap put
  // this rail here holding it. The strip paints its agents — their unread,
  // their working — on the first frame, and the read under way reconciles.
  let entity = railEntity(seedPayload(context), context.kind);
  let selectedId = openingAgentId(context, railView, entity.agents);
  let selectionFromMemory = reopenedBeforeAgentsKnown(context, entity.agents, selectedId);
  selection.set(selectedId);
  // ---- the project's agent, where the view asked for one --------------------
  // The rail stands on one conversation and keeps the other beside it: the
  // project's when this is the workspace, the workspace's when this is the
  // project. `alongside` is the one it is NOT standing on, read for the bubbles
  // of it the strip carries and for nothing else.
  // Whether the reader hears about this conversation (#65). One switch per
  // mounted rail, because the rail stands on one conversation at a time — a
  // swap to the project's side mounts a rail of its own with its own.
  let watchState = watchStateFor(context, entity.agents.find((agent) => agent.id === selectedId));
  /** A watch the bridge took, written into the cached row where nothing has
   *  written it since the ask: the strip keeps a watched agent from the answer
   *  on, not from whenever the push behind it lands (#105). The answer is the
   *  device's, not this mount's, so a reader who left before it came still
   *  finds it on return. A retired device's cache is left alone, asked inside
   *  the write itself: a device retired while the write was on its way to
   *  disk still finds nothing written. */
  const callWatch = async (method, params) => {
    const captured = await records.rowWrite(params.entity_id);
    const answer = await chatRepository.currentCall()(method, params);
    const watched = method === WATCH_VERB;
    await records.patchAgentIfUnwritten(captured, params.agent_id,
      (agent) => (cacheScope?.active() ? { ...agent, watched } : null));
    return answer;
  };
  const watchSwitch = createWatchToggle({
    ...watchState,
    entityId: () => context.entityId || entity.entityId || records.entityId(),
    agentId: () => selectedId,
    call: callWatch,
    onChange: (next, addressed) => {
      if (!standing() || addressed.agent_id !== selectedId) return;
      watchState = next;
      syncWatchButton(host.querySelector(WATCH_BUTTON_SELECTOR), next);
      // The press moves the open bubble's unwatched mark at once (#105); the
      // row the bridge pushes after the verb is what keeps it there.
      repaintStrip();
    },
    onFailure: (error, addressed) => {
      if (standing() && addressed.agent_id === selectedId) notifyError("Could not change watching", error.message || String(error));
    },
  });
  // When this rail's conversations compact (wire 1.10): what the bridge
  // answered, written into the cached row where nothing has written it since
  // the ask. Written once the menu has finished shutting — the row's watch
  // repaints the menu, and remounting it mid-animation would lose the movement.
  const compactionChoice = createCompactionChoice({
    call: (method, params) => chatRepository.currentCall()(method, params),
    capture: (entityId) => records.rowWrite(entityId),
    write: (captured, agentId, rewrite) => motionSettled().then(() =>
      standing() ? records.patchAgentIfUnwritten(captured, agentId, rewrite) : undefined),
    onFailure: (error) => {
      if (standing()) notifyError(COMPACTION_REFUSED, error.message || String(error));
    },
  });
  const { projectAgent, standing: onProjectAgentRail, entityId: knownOwner, name: knownName, projectId } = projectAgentState(context);
  let projectOwner = knownOwner;
  let projectName = knownName;
  let mintingProjectAgent = false;
  // The agent a URL named and this side has not accounted for yet.
  let wantedAgentId = context.openAgentId || null;
  let alongside = context.alongside && createAgentRailContext(context.alongside);
  let alongsideEntity = railEntity(seedPayload(context.alongside), alongsideKind(context));
  // Docked beside the work, or a card on the strip. The pin is the reader's
  // remembered layout choice; visibility belongs to this visit and never
  // rewrites that choice.
  let pinned = panelDocksByDefault();
  let panelVisible = panelStartsOut(context, pinned);
  let pinChoicePending = false;
  let pinnedKnown = false;
  // Which chat overview is open (#117): the project's, with every workspace in
  // it, or one workspace's. It opens as the page's own and moves only when the
  // reader moves it; the choice is cached per page so a remount — a swap to the
  // project's agent is one — keeps it.
  const overviewPage = overviewPageOf(context, alongside, projectId);
  let overviewScope = overviewPage.scope;
  let overviewRead = { rows: [], workspaces: [], issues: [] };
  const overviewScopeRecord = overviewPage.address ? watchUiState(overviewPage.address, (saved) => {
    if (!isOverviewScope(saved) || JSON.stringify(saved) === JSON.stringify(overviewScope)) return;
    overviewScope = saved;
    showOverviewScope();
  }) : NO_SCOPE_RECORD;
  const panelOut = () => panelVisible;
  let mode = railView.panelMode();
  let disposed = false;
  // A rail stands until it is disposed, or until the machine it was mounted
  // over is retired under it: that retires the machine's chat repository
  // (core/deviceContexts.js), and a read landing afterwards — its first
  // catalog read, the cached pin choice, a row — is about a scope nothing can
  // be minted in or asked through. It resolves to nothing and paints nothing;
  // the shell stands the route's rail up again over whatever context lands
  // next (core/shell.js).
  const standing = () => !disposed && chatRepository.active;
  const pinnedRecord = watchUiState(PINNED_ADDRESS, (saved) => {
    if (typeof saved?.pinned !== "boolean") return;
    pinned = saved.pinned;
    panelVisible = pinChoicePending ? true : panelStartsOut(context, pinned);
    pinChoicePending = false;
    pinnedKnown = true;
    paint();
  });
  void pinnedRecord.ready.then(() => {
    if (pinnedKnown || !standing()) return;
    pinnedKnown = true;
    paint();
  });
  // What this rail is listening to in the cache: the row it is the rail of, the
  // row of the conversation beside it, and the conversation in the panel.
  // Undefined until the rail has looked: `null` is an answer — this machine
  // holds no row for what the route names — and one the watch has to act on.
  let unwatchAlongsideRow = null;
  let watchedAlongsideId;
  let unwatchThread = null;
  let watchedThreadKey = null;
  let tui = null; // the mounted PTY pane, in TUI mode
  let titleMotion = null;
  const transientThreadCache = createThreadCache();
  let threadCache = transientThreadCache;
  // The rail's one selection (#148): `selectedId` says whose conversation, and
  // this says whether the panel shows it, the `+`'s chooser (left by the send
  // that creates the agent), or the chat overview. Every bubble on the strip
  // selects and toggles the same way (`pressBubble`).
  // What this mount opens on is remembered as it opens: a swap's press
  // selects as surely as a press below the line does.
  let selectedKind = openingKind(context, pageView);
  pageView.chooseKind(selectedKind);
  const addingAgent = () => selectedKind === "add";
  let threadAgentId = null; // whose conversation the cache holds
  let loadingOlderItems = false; // a page of history is in flight
  let olderItemsAwaitingPaint = false;
  let activityRuns = null; // the open runs of the conversation in the panel
  let activityRunsFor = null; // whose conversation those runs belong to
  let unreadFrom = null; // where the unread line stands in that conversation
  const unreadMarkers = new Map();
  let unreadMarker = null;
  const leaveUnreadMarker = () => {
    unreadMarker?.leave();
    unreadFrom = null;
  };
  let reportedRead = 0; // how far this panel has told the daemon it read
  let reportedFloor = null; // and how much of the conversation it held saying so
  let paintedChat = null; // what the timeline in the panel was drawn from
  let paintedChatContent = null; // drawn rows, including same-cursor edits
  let paintedChatItemCount = 0;
  let paintedDigests = []; // the run totals that timeline was drawn with
  let paintedEntryKeyOf = () => null; // which of its entries draws a sequence
  // How much of the held conversation the timeline draws (#158): the newest
  // entries, and more as the reader asks for them.
  const timelineSlice = createTimelineSlice();
  let seededSurfaces = null;
  const chatOwnership = createRailChatOwnership(chatRepository, key, () => entity);
  // An optimistic agent is the provisional controller gaining a visible card,
  // not a second conversation. Keep its draft, pending sends, and eventual
  // resolved identity on the controller that created it.
  const controllerForAgent = (agent) => isProvisionalKey(agent?.id) ? null : chatOwnership.controllerFor(agent);
  const provisionalController = () => chatOwnership.provisional();

  const cacheIdentity = () => {
    if (!standing()) return null;
    const addressed = addressedCacheIdentity({
      cacheScope,
      context,
      railEntityId: entity.entityId || records.entityId(),
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
      if (!standing()) return;
      threadAgentId = seededFor;
      // Edits and local delivery changes can keep the same cursor/count.
      // Compare only the drawn tail, preserving the unchanged-record fast path.
      if (paintedChat !== null && loadedConversationItems().length === paintedChatItemCount
        && paintedChatContent !== drawnChatContent()) paintedChat = null;
      paintChat();
      // The line above the composer is drawn from the conversation too — what
      // the agent is doing, and what started it — so a window arriving moves
      // it as much as it moves the timeline.
      paintRailStatus();
    },
    onSurfacesSeeded: (seen) => {
      if (!standing()) return;
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
    seededSurfaces = null;
    return conversationCache;
  };

  const resetConversationCache = () => {
    conversationCache = null;
    seededSurfaces = null;
    olderItemsAwaitingPaint = false;
  };
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
  /** This machine's whole view of the feed, which is how an agent of ANOTHER
   *  workspace gets a name: a tracking notice is about somebody else's act
   *  (core/trackerNotice.js), so the row this rail stands on is not enough. */
  let feedView = null;
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
  const disposeComposerModelMenu = () => composerModelMenu?.dispose();
  let composerGauge = null;
  let composerController = null;
  let unsubscribeComposerController = null;
  let surfacesBlock = null;
  let observationBlock = null;
  let issuesBlock = null;
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
  /** The agents as the strip carries them (#105): the open one watched as its
   *  switch says, which moves under the press before the row does. */
  const stripAgents = () => visibleAgents().map((agent) => (agent.id === selectedId && watchState
    ? { ...agent, watched: watchState.watching } : agent));
  /** The conversation a read settles on: the one open while it still exists,
   *  else the first watched. One reopened from memory before the agents were
   *  known is dropped here if the reader does not watch it. */
  const settleSelection = () => {
    const agents = visibleAgents();
    const wanted = selectionFromMemory ? rememberedAgentId(agents, selectedId) : selectedId;
    if (agents.length) selectionFromMemory = false;
    chooseAgent(selectAgentId(agents, wanted));
  };
  const syncWatchFromAgent = () => {
    const current = watchStateFor(context, agentOf(selectedId));
    if (current) watchSwitch.settle(current);
    else watchState = null;
  };
  /** Which kind of selection the panel shows, remembered with the rail's view
   *  so a remount comes back to it (#148). */
  const selectKind = (kind) => {
    selectedKind = kind;
    pageView.chooseKind(kind);
  };
  /** Open this agent's conversation, and tell everything else on screen: the
   *  bubble strip is the selector for the whole work item, not just the rail. */
  const chooseAgent = (id) => {
    if (selectedId !== (id || null)) leaveUnreadMarker();
    selectedId = id || null;
    syncWatchFromAgent();
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

  /** What a detail level is remembered against: the conversation's own id, or —
   *  for an agent that has not been born yet, and so has no conversation to
   *  have an id — the key its draft is held under, which the real id replaces
   *  the moment the daemon answers with one. */
  const conversationDetailId = (controller = controllerInFocus()) =>
    controller.identity.conversationId || conversationKey(controller);

  /** How much of each conversation this panel has been asked to draw.
   *
   *  Held as well as stored, because the toggle has to take effect on the press
   *  rather than on whatever tick reads storage next — and because the rail
   *  moves between conversations without being torn down, so one variable
   *  would carry the last conversation's choice into the next one.
   */
  const chosenDetailLevels = new Map();
  const detailRecords = new Map();

  const detailRecord = (id) => {
    if (detailRecords.has(id)) return detailRecords.get(id);
    const address = uiAddress({
      deviceId: cacheScope?.deviceId || "",
      entityId: entity.entityId || "",
      view: "thread",
      kind: "filter",
      sub: id,
    });
    const record = watchUiState(address, (saved) => {
      if (!saved || !["all", "messages", "agent"].includes(saved.level)) return;
      chosenDetailLevels.set(id, saved.level);
      paintedChat = null;
      paintChat();
      paintSurfaceMenu();
    });
    detailRecords.set(id, record);
    return record;
  };

  const detailLevel = () => {
    const id = conversationDetailId();
    if (!chosenDetailLevels.has(id)) chosenDetailLevels.set(id, defaultDetailLevel(entity.kind));
    detailRecord(id);
    return chosenDetailLevels.get(id);
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
    const params = modelParams(models || [], choice.model || choice.requestedModel, choice.effort, choice.provider);
    // Omitted when the reader typed nothing, so the bridge's own rule stands:
    // an unnamed agent is asked to name itself the first time it is written to.
    const name = newAgentName();
    const work = newAgentWork();
    return {
      ...params,
      ...(name ? { name } : {}),
      // Omitted when the reader chose no role, so the device default stands
      // rather than being spelled out here.
      ...(work.role ? { role: work.role } : {}),
    };
  };

  /** What the reader said this agent is for. Kept beside the name, and for the
   *  same reason: it belongs to the one agent about to be made. */
  let pendingWork = { role: "" };
  const newAgentWork = () => pendingWork;
  const writeNewAgentWork = (field, value) => {
    pendingWork = { ...pendingWork, [field]: String(value || "") };
  };

  /** What the machine says its models are for, for the picker's preview. It
   *  rides the catalog (`models.list`), which this rail already reads, so the
   *  preview costs no call of its own — and a rail opening a cached
   *  conversation still asks the bridge for nothing but that. */
  const roleModelsOf = (offered) => (Array.isArray(offered?.role_models) ? offered.role_models : []);

  /** What the reader typed into the new-agent name field, if anything. Kept on
   *  the rail rather than in the choice, because it belongs to the one agent
   *  about to be made and not to the harness that will run it. */
  let pendingAgentName = "";
  const newAgentName = () => pendingAgentName.trim();
  const writeNewAgentName = (word) => {
    pendingAgentName = String(word || "").slice(0, 24);
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
    if (!panelVisible || !standing()) return;
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

  // ---- reading the work item ------------------------------------------------
  //
  // From the cache and nothing else, for every kind of work item the board
  // writes a row for. The row this rail is the rail of carries its agents — who
  // they are, what they are doing, what they have observed — and the sync layer
  // keeps that row current from the board's pushes. So the strip and the panel
  // paint on the first frame, from disk, and move when the record moves rather
  // than when a poll comes back. Which record that is, and hearing it move, is
  // core/railWorkItem.js.

  const records = createRailWorkItem({
    context,
    railContext,
    cacheScope,
    callFor: () => chatRepository.currentCall(),
    named: (name) => swap?.named(name),
    standOn: (row) => standOnRow(row),
    reread: () => refresh(),
    // The paint fingerprint is over the conversation, and the conversation did
    // not move — only what its file chips are addressed against did.
    redrawConversation: () => {
      paintedChat = null;
      paintChat();
    },
    alive: standing,
  });

  const workspaceNameFor = (workspaceId) =>
    workspaceDisplayName(feedView?.workspaces?.find((workspace) =>
      (workspace.workspace_id || workspace.id) === workspaceId), workspaceId || "Workspace");
  const overviewSource = (slot, standing, entityId) => ({
    slot, kind: standing.kind, entityId,
    section: standing.kind === "project" ? "project" : standing.kind === "workspace" ? "workspace" : "other",
    sectionName: standing.kind === "project" ? "Project agents"
      : standing.kind === "workspace" ? workspaceNameFor(standing.workspaceId) : "Agents",
    workspaceId: standing.kind === "workspace" ? standing.workspaceId : null,
    address: records.rowAddress(entityId),
  });

  /** Whether the overview needs every workspace of the project read: for the
   *  project's scope, and for a workspace other than the page's own. */
  const overviewReadsWorkspaces = () => overviewScope.kind === "project"
    || (overviewScope.kind === "workspace" && overviewScope.workspaceId !== overviewPage.workspaceId)
    || (overviewScope.kind === "page" && overviewPage.kind === "project");

  const overview = createAgentOverview({
    scope: cacheScope,
    projectId,
    includeProjectWorkspaces: overviewReadsWorkspaces,
    sources: () => [overviewSource("current", context, records.entityId() || entity.entityId),
      ...(alongside ? [overviewSource("alongside", alongside, watchedAlongsideId)] : [])],
    onRows: (rows, { workspaces, issues }) => paintOverviewRows({ rows, workspaces, issues }),
  });

  const answerLostTheAgents = (answered) => {
    if (answered.agents.length || !visibleAgents().length || agentlessOnce) return false;
    agentlessOnce = true;
    return true;
  };

  const showTaskCompletions = (agents) => {
    completionTracker.forgetMissing(agents.map((agent) => agent.id));
    for (const agent of agents) {
      const completions = completionTracker.observe(agent);
      if (completions.length) completionToast.show(agent.id, completions.map((task) => task.title));
    }
  };

  /// The row as the rail reads it: the work item the board pushed, widened by
  /// what only the workspace list knows — the sources the workspace is mounted
  /// out of, and so the branch it is on.
  const rowAsRead = (row) => {
    const sources = records.sources();
    return context.kind === "workspace" && sources.length ? { ...row, directories: sources } : row;
  };

  /// What the row says, taken up by the rail: the agents on the strip, whose
  /// conversation is open, and the controllers behind them.
  const standOnRow = (pushed) => {
    const row = rowAsRead(pushed);
    const answered = railEntity(row, context.kind);
    if (answerLostTheAgents(answered)) return;
    // A start that never reached a harness is answered here and nowhere else:
    // the daemon replied to the press long before the spawn, so the row is the
    // first word about it.
    for (const failed of startFailuresLearned(entity.agents, answered.agents)) {
      notifyError("Could not start the agent", agentStartFailure(failed));
    }
    agentlessOnce = false;
    entity = answered;
    keepForSwap(context.kind, row);
    for (const agent of answered.agents) controllerForAgent(agent);
    reconcileOptimistic(pendingAgentsScope(), answered.agents, { keyOf: agentIdOf });
    settleSelection();
    controllerForAgent(agentOf(selectedId))?.reconcileUncertain().then(syncChatRecovery);
    paint();
    overview.refresh();
    showTaskCompletions(answered.agents);
  };

  /// The rail's own work item, read again. Called on mount, whenever the record
  /// moves, and after a mutation this rail made — a verb answers before the push
  /// that says what it did, and the record is where that lands.
  const refresh = async () => {
    const entityId = await records.entityIdFor(railContext);
    if (!standing()) return;
    const ownerChanged = records.entityId() !== entityId;
    records.watch(entityId);
    // A workspace's cached list can name its conversation owner before the
    // cached work-item row carrying the roster arrives. The first panel paint
    // already attempted the remembered conversation without that owner; point
    // the cache reader at it again now that its address is complete.
    if (ownerChanged) {
      resetConversationCache();
      threadAgentId = null;
    }
    paintChat();
    const row = await records.read(entityId);
    if (!standing()) return;
    if (row) standOnRow(row);
    await refreshAlongside();
  };

  /// Stop listening to the cache: this rail's row and the list under it, the
  /// row of the conversation beside it, and the conversation in the panel.
  const unwatchCache = () => {
    records.unwatch();
    unwatchAlongsideRow?.();
    unwatchAlongsideRow = null;
    unwatchThread?.();
    unwatchThread = null;
  };

  // ---- the conversation beside this one -------------------------------------

  /// What this rail has just read, for the mount that holds its two contexts to
  /// keep: the other side of a swap is stood up on it, so the strip it paints
  /// says what this side is doing rather than starting again from nothing.
  const keepForSwap = (kind, payload) => swap?.read(kind, payload);

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

  /// The project's own row in the cached project list, or nothing when that
  /// machine has no such project.
  const listedProjectRow = (listed) =>
    (listed || []).find((project) => project.project_id === projectId);

  /// The project's row out of the cached project list — the one read that
  /// names a project without minting anything on it.
  const listedProject = async () => {
    const address = cacheScope?.address({ entityId: "", kind: PROJECTS_RECORD_KIND });
    return address ? listedProjectRow((await readCached(address))?.value) : null;
  };

  /// The project's own page asks that list for one thing only — the name its
  /// bubbles cut their initial from. Its owner is the conversation this rail
  /// is already standing on, and there is nothing beside it to refresh.
  const readProjectPageName = async () => {
    if (projectAgent || !projectId) return;
    const row = await listedProject();
    if (!standing() || !row?.name) return;
    projectName = row.name;
    paint();
  };

  /// What the project above this work item is called, and whether it has a
  /// conversation yet. The project list is the only read that answers the
  /// second without minting one (planning/v2/workspaces.md), and it answers the
  /// first in the same breath — and the sync layer keeps it on disk, so this is
  /// a read of the cache like every other read the rail makes. A bubble waiting
  /// to be started is a true thing to show until the press that mints one.
  const readProjectAgent = async () => {
    if (!projectAgent || (projectOwner && projectName)) return;
    const row = await listedProject();
    if (!standing() || !row) return;
    learnProjectFacts({ entityId: row.entity_id || row.run_id, name: row.name });
    paint();
    await refreshAlongside();
  };

  /// What this machine says a new project agent starts on, asked once and only
  /// where it is spent: the rail standing on a project's conversation. A rail
  /// on a work item never mints a project agent, so it never asks.
  ///
  /// A refusal reads as no preference, which is what a machine that will not
  /// answer offers — the catalog's own default harness then stands, exactly as
  /// it does for a device that has chosen nothing.
  const readProjectAgentSetting = async () => {
    if (!onProjectRail() || !standing()) return null;
    const call = chatRepository.currentCall();
    const settings = await call("settings.get", {}).catch(() => null);
    return settings && projectAgentChoiceOf(settings);
  };

  /// The conversation this rail is not standing on — the project's from a
  /// workspace, the workspace's from inside the project's — for what its
  /// bubbles say. Its row, out of the same cache this rail's own comes from.
  /// There is nothing to read while the project has no owner: that bubble says
  /// how to start one instead, which is not a read.
  const refreshAlongside = async () => {
    if (!alongside || (alongside.kind === "project" && !projectOwner)) return;
    const entityId = await records.entityIdFor(alongside);
    if (!standing()) return;
    watchAlongsideRow(entityId);
    // A row this device does not hold leaves the strip saying what it said: a
    // bubble that blanks because one record is cold is worse than one behind.
    const row = await records.cachedRow(entityId);
    if (!standing() || !row) return;
    alongsideEntity = railEntity(row, alongside.kind);
    keepForSwap(alongside.kind, row);
    paint();
    overview.refresh();
    standOnNamedConversation();
  };

  /// Hear the other side's row move too: both halves of the strip say what
  /// their agents are doing, and only one of them is this rail's own.
  const watchAlongsideRow = (entityId) => {
    if (watchedAlongsideId === entityId) return;
    unwatchAlongsideRow?.();
    unwatchAlongsideRow = null;
    watchedAlongsideId = entityId;
    const address = records.rowAddress(entityId);
    if (!address) return;
    unwatchAlongsideRow = subscribeCache(address, () => void refreshAlongside());
  };

  /// The conversation a URL named, when it turns out to be the one across the
  /// line: the project's agent is reachable from every workspace in the
  /// project, so a workspace page may be asked to open a conversation that is
  /// not the workspace's own. Asked once, of the first read of the other side —
  /// after that the rail is wherever the reader has put it.
  const standOnNamedConversation = () => {
    if (!wantedAgentId || onProjectAgentRail || !swap || !projectOwner) return;
    const wanted = wantedAgentId;
    wantedAgentId = null;
    if (agentOf(wanted) || !alongsideEntity.agents.some((agent) => agent.id === wanted)) return;
    swap.toProject(projectOwner, wanted);
  };

  /// The project's own page, on the machine this rail is mounted on.
  const projectPageRoute = () => ({
    name: "project",
    deviceId: context.deviceId ?? null,
    projectId: projectAgent.projectId,
  });

  /// The chip on the panel's head, or null where there is nothing for it to
  /// say: the rail is standing on the project's conversation, and it got here
  /// from a work item whose page is still behind the panel. On the project's
  /// own page the chip would point at the page it is on, which is not a link.
  const projectChip = () =>
    (swap && onProjectAgentRail ? { name: projectName || projectAgent.projectId, route: projectPageRoute() } : null);

  /// The work item under the rule — the workspace or the branch this rail is
  /// the rail of, on whichever side of the swap it sits. It answers for its own
  /// half of the strip: its agents, and whether another can be added to it.
  const belowTheLine = () => (onProjectAgentRail ? alongsideEntity : entity);

  /// The project's bubble, or null on a rail nobody asked for one on. Its
  /// agents are whichever side of the swap the project is on.
  const projectAgentEntry = () =>
    projectAgent && {
      name: projectName || projectAgent.projectId,
      entityId: projectOwner,
      agents: onProjectAgentRail ? stripAgents() : alongsideEntity.agents,
      active: onProjectAgentRail && selectedKind === "agent",
      openAgentId: onProjectAgentRail ? selectedId : null,
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
    if (type === "agent" && agentId && !isSelected({ kind: "agent", agentId })) {
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

  const openOverviewRow = (button) => {
    const agentId = button.dataset.overviewAgent;
    const source = button.dataset.overviewSource;
    const workspaceId = button.dataset.overviewWorkspace;
    if (source === "workspace" && workspaceId) {
      go({ name: "workspace", deviceId: context.deviceId, projectId, workspaceId, tab: "changes", agent: agentId });
      return;
    }
    if (source === "alongside" && swap) {
      if (onProjectAgentRail) swap.toWorkItem(agentId);
      else swap.toProject(projectOwner, agentId);
      return;
    }
    openAgent(agentId);
    paint();
  };

  const openOverviewAdd = (workspaceId) => {
    if (workspaceId === context.workspaceId && !onProjectAgentRail) {
      openNewAgent();
      return;
    }
    if (workspaceId === alongside?.workspaceId && onProjectAgentRail && swap) {
      swap.toWorkItem(null, { adding: true });
      return;
    }
    go({ name: "workspace", deviceId: context.deviceId, projectId, workspaceId, tab: "changes", newAgent: true });
  };

  const paintOverviewRows = (read) => {
    overviewRead = read;
    const list = host.querySelector(".rail-overview-list");
    if (!list) return;
    const markup = overviewHtml(read.rows, { showProjectAgents: !!projectId, scope: overviewScope,
      workspaces: read.workspaces, issues: read.issues || [] });
    if (list.innerHTML !== markup) list.innerHTML = markup;
    list.onclick = (event) => {
      const add = event.target.closest?.("[data-overview-add]");
      if (add) { openOverviewAdd(add.dataset.overviewAdd); return; }
      const opens = event.target.closest?.("[data-overview-scope]");
      if (opens) { moveOverviewScope({ kind: "workspace", workspaceId: opens.dataset.overviewScope }); return; }
      const button = event.target.closest?.("[data-overview-agent]");
      if (button) openOverviewRow(button);
    };
  };

  /** The head's way out to the project's overview, there only while the
   *  overview is one workspace's. */
  const syncOverviewHead = () => {
    const head = host.querySelector(`${OVERVIEW_PANEL} .rail-head`);
    if (!head) return;
    const wanted = overviewScope.kind === "workspace";
    const present = head.querySelector(".rail-overview-up");
    if (!wanted) { present?.remove(); return; }
    if (present) return;
    head.querySelector(".rail-who").insertAdjacentHTML("afterend",
      scopeLinkHtml({ label: "All workspaces", className: "rail-overview-up" }));
    head.querySelector(".rail-overview-up").onclick = () => moveOverviewScope({ kind: "project" });
  };

  /** Redraw the open overview for the scope it is on now: what is already read
   *  at once, then whatever the new breadth has to read. */
  const showOverviewScope = () => {
    syncOverviewHead();
    paintOverviewRows(overviewRead);
    overview.refresh();
  };

  /** The reader moved the overview between the project and one workspace. The
   *  press that did it is gone with the redraw, so focus goes to the list it
   *  redrew rather than to the document. */
  const moveOverviewScope = (scope) => {
    overviewScope = scope;
    void overviewScopeRecord.write(scope);
    showOverviewScope();
    host.querySelector(".rail-overview-list")?.focus();
  };

  /** The overview as the one panel's body (#148): a head that says "Agents"
   *  with only the pin, over the rows last read, which its own read then moves.
   *  Built once per visit; the conversation's body is rebuilt on the way back. */
  const paintOverviewPanel = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    if (panel.dataset.body !== OVERVIEW_BODY) {
      disposeTitleMotion();
      disposeTui();
      releaseConversationChrome();
      closeSurfaceOverlay();
      leaveUnreadMarker();
      panel.innerHTML = `${panelHeadHtml("Agents", "chat", { hasTerminal: false, showHarnessIcon: false, pinned })}<div class="rail-overview-list" tabindex="-1"></div>`;
      Object.assign(panel.dataset, { body: OVERVIEW_BODY, head: "", title: "", conversation: "" });
      wireHead(panel);
      syncOverviewHead();
      paintOverviewRows(overviewRead);
    }
    syncPinButton(panel.querySelector(`.${PIN_CLASS}`), { subject: PANEL_SUBJECT, pinned });
  };

  /** The overview reads only while it is what the panel is showing. */
  const syncOverviewRead = () => {
    if (panelVisible && selectedKind === "overview") overview.open();
    else overview.close();
  };

  const stripBubbles = () => {
    const below = belowTheLine();
    const bubbles = railBubbles({
      // Below the line are the work item's agents, whichever side of the swap
      // this rail is standing on.
      agents: onProjectAgentRail ? alongsideEntity.agents : stripAgents(),
      selectedId, selectedKind, kind: below.kind, chatCapable: below.chatCapable !== false,
      canAdd: onProjectAgentRail ? below.canAdd : null,
      projectAgent: projectAgentEntry(),
      projectName,
    });
    const overviewBubble = { type: "overview", id: "", title: "Chat overview",
      active: selectedKind === "overview", label: "", unread: 0, working: false };
    const opened = (bubble) => ({ ...bubble, expanded: panelVisible && Boolean(bubble.active) });
    return [...bubbles, overviewBubble].map(opened);
  };

  /** The strip alone, for a change that moves nothing else on the rail. */
  const repaintStrip = () => {
    const strip = standing() && host.querySelector(".rail-strip");
    if (strip) paintStrip(strip, stripBubbles());
  };

  const paint = () => {
    if (!standing()) return;
    if (!host.querySelector(".rail-strip")) {
      releaseFaces();
      host.innerHTML = `<div class="rail-strip"></div>`;
    }
    repaintStrip();
    // The strip is useful immediately; the panel waits for its cached pin
    // choice so a remount never flashes the wrong layout or creates a panel
    // that then has to be discarded.
    if (!pinnedKnown) return;
    let panel = host.querySelector("#rail-panel");
    if (panelOut() && !panel) {
      panel = document.createElement("div");
      panel.className = "rail-panel";
      panel.id = "rail-panel";
      host.insertBefore(panel, host.querySelector(".rail-strip"));
    }
    if (panelOut()) paintPanel();
    syncPopover();
    syncOverviewRead();
  };

  // ---- docked, or a card on the strip ---------------------------------------

  /** Which bubble the popover points at, and where its notch sits along the
   *  panel's edge to point there — down the panel on a desktop, across its foot
   *  on a phone, because that is which way the strip runs. */
  const anchorPopover = (panel, showing) => {
    const bubble = showing ? activeBubble() : null;
    setData(panel, "anchor", bubble ? bubble.dataset.agent || bubble.dataset.bubble : "");
    if (!bubble) return panel.style.removeProperty(ANCHOR_PROPERTY);
    const box = bubble.getBoundingClientRect();
    const frame = panel.getBoundingClientRect();
    const offset = stripRunsAcross()
      ? box.left + box.width / 2 - frame.left
      : box.top + box.height / 2 - frame.top;
    panel.style.setProperty(ANCHOR_PROPERTY, `${Math.round(offset)}px`);
  };

  const activeBubble = () => host.querySelector(`${STRIP_BUTTONS}.active`);

  const syncPopover = () => {
    const card = !pinned;
    host.classList.toggle("rail-unpinned", !pinned);
    host.classList.toggle(COLLAPSED_CLASS, !panelVisible);
    host.classList.toggle(POPOVER_CLASS, card);
    const panel = host.querySelector("#rail-panel");
    if (panel) {
      setAttr(panel, "aria-hidden", !panelVisible);
      panel.toggleAttribute("inert", !panelVisible);
      anchorPopover(panel, card);
    }
    host.querySelectorAll(STRIP_BUTTONS).forEach((bubble) => {
      setAttr(bubble, "aria-expanded", panelVisible && bubble.classList.contains("active"));
      setAttr(bubble, "aria-controls", "rail-panel");
    });
  };

  /** Dock the panel, or let it go. Unpinning leaves the conversation on screen
   *  as the popover it becomes — the same move the inbox's pin makes
   *  (core/inboxShell.js) — unless the press was a way of shutting it. */
  const setPinned = (on) => {
    pinChoicePending = true;
    return pinnedRecord.write({ pinned: on });
  };

  /** Collapse the live panel without changing its docked/card preference. */
  const closePanel = ({ restoreFocus = false } = {}) => {
    if (!panelVisible) return;
    leaveUnreadMarker();
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
    selectedKind === "agent" && !!selectedId && !isProvisionalKey(selectedId) && !agentInFocus();

  const conversationIsUnselected = () =>
    !visibleAgents().length && (!selectedId || isProvisionalKey(selectedId));

  const panelBodyIdentity = () => {
    if (addingAgent()) return "new";
    // Every existing agent uses the same mounted conversation frame. Moving
    // between bubbles changes what the shared thread painter reconciles into
    // that frame; it is not a reason to tear the whole panel down first.
    return "conversation";
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
    if (!panel) return;
    panel.dataset.body = controller
      ? `${shownPanelMode()}:conversation`
      : wantedPanelBody();
    if (controller) panel.dataset.conversation = conversationKey(controller);
  };

  const releaseConversationChrome = () => {
    disposeSurfaces();
    disposeComposerClearance?.();
    disposeComposerClearance = null;
    closeSurfaceMenu?.();
    composerControl?.dispose?.();
    disposeComposerModelMenu();
    unsubscribeComposerController?.();
    unsubscribeComposerController = null;
    composerController = null;
    composerControl = null;
    composerModelMenu = null;
    composerGauge = null;
  };

  /** Move the pinned controls to another conversation without touching the
   *  timeline above them. Drafts, uploads and surface state belong to the
   *  controller; the thread frame belongs to the one shared painter. */
  const rebindConversationChrome = (panel) => {
    const standing = panel.querySelector("#rail-footer");
    if (!standing) return;
    releaseConversationChrome();
    standing.outerHTML = composerRowHtml();
    wireComposer(panel);
    deliverDroppedFiles();
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 16, cap 10 — reduce it, then drop this line
  const paintConversationPanel = () => {
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
    if (shownMode !== "chat" || addingAgent() || rememberedConversationIsLoading()) leaveUnreadMarker();
    // Structural head changes replace its controls; a topic change keeps this
    // head mounted and moves only its title below — so the head is fingerprinted
    // by WHICH agent it is open on rather than by what that agent is called.
    // The id is what tells two starting agents on one harness apart: without it
    // a strip of two unnamed Claude Codes has one fingerprint, and switching
    // between them would leave the first one's head standing.
    const chip = projectChip();
    const wantedHead = JSON.stringify([agent ? agent.id : "", provider, removable, hasTerminal, chipMark(chip)]);
    const wantedTitle = JSON.stringify([who, heading.text, heading.starting]);
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = wantedPanelBody();
    const wantedConversation = shownMode === "chat" && !addingAgent() && !rememberedConversationIsLoading()
      ? conversationKey()
      : "";
    const conversationChanged = panel.dataset.conversation !== wantedConversation;
    if (panel.dataset.body !== wantedBody) {
      disposeTitleMotion();
      disposeTui();
      releaseConversationChrome();
      panel.innerHTML = `${panelHeadHtml(who, shownMode, { provider, removable, hasTerminal, menuGroups: surfaceMenuGroupsInFocus(), heading, pinned, removalWho, projectChip: chip, watch: watchState })}
        <div class="rail-body" id="rail-body"></div>
        ${shownMode === "chat"
          ? `${rememberedConversationIsLoading() || entity.chatCapable === false ? "" : composerRowHtml()}`
          : ""}`;
      panel.dataset.head = wantedHead;
      panel.dataset.title = wantedTitle;
      panel.dataset.body = wantedBody;
      panel.dataset.conversation = wantedConversation;
      mountTitleMotion(panel);
      wireHead(panel);
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
        watch: watchState,
        provider,
        removable,
        hasTerminal,
        menuGroups: surfaceMenuGroupsInFocus(),
        heading,
        pinned,
        removalWho,
        projectChip: chip,
      });
      panel.dataset.head = wantedHead;
      panel.dataset.title = wantedTitle;
      mountTitleMotion(panel);
      wireHead(panel);
    }
    if (panel.dataset.body === wantedBody
      && conversationChanged
      && panel.dataset.conversation !== wantedConversation
      && wantedConversation) {
      if (panel.querySelector("#rail-footer")) rebindConversationChrome(panel);
      else {
        panel.insertAdjacentHTML("beforeend", composerRowHtml());
        wireComposer(panel);
        deliverDroppedFiles();
      }
      panel.dataset.conversation = wantedConversation;
    }
    syncPanelTitle(panel, wantedTitle, { text: heading.text, title: who, starting: heading.starting });
    // The remove button names the agent by its topic too, and a topic arriving
    // is not a structural change: it re-reads on the title's beat rather than
    // waiting for a head the rail has no reason to rebuild.
    syncRemoveWording(panel, removalWho);
    const pin = panel.querySelector(`.${PIN_CLASS}`);
    if (pin) syncPinButton(pin, { subject: PANEL_SUBJECT, pinned });
    // Written onto the standing button rather than fingerprinted into the head:
    // a press moves this and nothing else, and rebuilding the head for it would
    // take the surface menu and the title's motion with it.
    if (watchState) syncWatchButton(panel.querySelector(WATCH_BUTTON_SELECTOR), watchState);
    paintSurfaceMenu();
    syncSurfaceOverlay();
    if (shownMode === "chat") {
      paintChat();
      paintRailStatus();
    }
    usageLimitBanner.sync();
  };

  /** The panel, painted for whatever the rail has selected. */
  const paintPanel = () => (selectedKind === "overview" ? paintOverviewPanel() : paintConversationPanel());

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
    const watch = panel.querySelector(WATCH_BUTTON_SELECTOR);
    if (watch) watch.onclick = () => watchSwitch.press();
    const chip = panel.querySelector(".rail-project-chip");
    if (chip) chip.onclick = (event) => {
      // The anchor's href is the reader's to open elsewhere; a plain press is
      // the app's, and goes there without a page load.
      if (pressIsTheBrowsers(event)) return;
      event.preventDefault();
      go(projectPageRoute());
    };
    const pin = panel.querySelector(`.${PIN_CLASS}`);
    if (pin) pin.onclick = () => panelMotion.run({
      panel,
      direction: pinned ? "popover" : "pinned",
      scroller: panel.querySelector(".rail-body"),
      apply: () => {
        void setPinned(!pinned);
      },
    });
  };

  // ---- chat -----------------------------------------------------------------

  /// The conversation on screen: the window the record holds, opened into the
  /// cache the panel paints from.
  ///
  /// Pressing another bubble switches which conversation that is, and the
  /// switch is what re-points the reader at the other record — and at the
  /// controller that owns it, since every conversation keeps its own window
  /// across a remount (core/chatRepository.js).
  const threadWindow = () => {
    if (threadAgentId !== selectedId) {
      resetConversationCache();
      threadAgentId = selectedId;
      // Fire and forget: the seed paints when it lands, and until it does the
      // panel shows the conversation it is already holding rather than a gap.
      void watchConversation();
    }
    const held = threadCache.readWindow();
    return held ? { items: held.items, activityDigests: held.activityDigests } : null;
  };

  /// Open the record this conversation is held in, and hear it move. Every
  /// write to it — a page the sync layer pulled, a push it applied, a message
  /// this panel just sent — is a repaint from the record and nothing else.
  const watchConversation = async () => {
    const conversation = bindConversationCache();
    const address = conversation.address();
    if (watchedThreadKey !== threadAddressKey(address)) {
      unwatchThread?.();
      unwatchThread = null;
      watchedThreadKey = threadAddressKey(address);
      if (address) unwatchThread = subscribeCache(address, () => void conversation.reread());
    }
    if (await conversation.seed()) return;
    const identity = cacheIdentity();
    const askedKey = threadAddressKey(address);
    if (!identity || !address || !askedKey) return;
    await syncThreadWindow({
      deviceId: identity.deviceId,
      call: chatRepository.currentCall(),
      active: () => standing() && askedKey === threadAddressKey(conversation.address()),
      entityId: identity.entityId,
      agentId: identity.agentId,
      conversationId: identity.conversationId,
      address,
      priority: "foreground",
    });
  };

  const threadFor = () => {
    const scope = pendingThreadScope(selectedId);
    const thread = threadWindow();
    const held = (thread && thread.items) || [];
    // The agent this conversation belongs to may not exist yet — the first
    // message on a work item creates it — and until it does there is no record
    // to write to. That one message is projected over the window instead.
    reconcileOptimistic(scope, held, { keyOf: threadItemKey });
    const items = projectOptimistic(scope, held, { keyOf: threadItemKey });
    if (thread) return { ...thread, items };
    return items.length ? { items } : null;
  };

  /// Ask for the conversation above the window the reader is standing at the
  /// top of.
  ///
  /// A long conversation is held as a window over its newest items, so the top
  /// of the scroller is a floor rather than the start, and this is what lifts
  /// it. The page is written into the record (core/agentRailContext.js), which
  /// is what repaints the panel — the one read this rail makes, landing where
  /// every other write to the conversation lands. One page in flight at a time:
  /// a scroll gesture fires the handler many times over, and each of those
  /// would otherwise be a round trip for the same history.
  const olderReadRequest = () => {
    if (!panelVisible) return null;
    const seek = threadCache.olderPageParam();
    if (loadingOlderItems || !seek || !threadCache.hasOlderItems() || !entity.entityId) return null;
    const call = chatRepository.currentCall();
    const addressed = controllerForAgent(agentOf(selectedId));
    return { seek, call, addressed, asked: selectedId, address: bindConversationCache().address() };
  };

  const fetchOlderPage = async ({ call, addressed, seek, asked, address }) => {
    try {
      return await railContext.olderPage(call, {
        entityId: addressed?.identity.entityId || entity.entityId,
        agentId: addressed?.identity.agentId || asked,
        beforeSequence: seek.before_sequence,
        address,
      });
    } catch (error) {
      notifyError("Could not load older messages", error.message);
      return null;
    }
  };

  /// Show the reader the conversation above what the timeline draws: the next
  /// entries the cache holds, drawn at once, and once it holds no more, the
  /// page above it from the bridge.
  const showEarlierEntries = () => {
    if (!timelineSlice.hasHiddenEntries()) {
      readOlderItems();
      return;
    }
    timelineSlice.showEarlier();
    paintChat({ olderItemsPrepended: true });
  };

  /// Draw the timeline down to `sequence` if it stops short of it, so what is
  /// about to be gone to — a deep link's row, a tick's message — is there.
  ///
  /// Whether it is drawn is whether the entry that owns it is: its own row, or
  /// the run the held items say it is folded into. Never whether some drawn
  /// run's numbers cover it, since a subagent's calls fold under the call that
  /// spawned them wherever they land and one run's span can reach across
  /// another's (#158).
  const drawDownTo = (sequence) => {
    const body = host.querySelector("#rail-body");
    const wanted = Number(sequence);
    if (!body || !Number.isFinite(wanted)) return;
    const key = paintedEntryKeyOf(wanted);
    if (key === null) {
      drawDownToUnheld(body, wanted);
      return;
    }
    if (!body.querySelector(`.thread-items > [data-key="${key}"]`)) reachDownTo(Number(key));
  };

  /// A sequence the window does not hold has no owner in hand to go by. It is a
  /// row an open run fetched, or it sits in the cut half of a run whose digest
  /// reaches back over it, or it is older than anything held and has no row to
  /// draw down to.
  const drawDownToUnheld = (body, wanted) => {
    if (cutRunKeyAt(body, wanted) || body.querySelector(`.thread-items [data-sequence="${wanted}"]`)) return;
    const heldFrom = threadCache.windowFloorSequence() ?? wanted;
    if (wanted < heldFrom && !digestCovering(paintedDigests, wanted)) return;
    reachDownTo(Math.max(wanted, heldFrom));
  };

  const reachDownTo = (sequence) => {
    timelineSlice.reachDown(sequence);
    paintChat({ olderItemsPrepended: true });
  };

  const readOlderItems = async () => {
    const request = olderReadRequest();
    if (!request) return;
    loadingOlderItems = true;
    const page = await fetchOlderPage(request);
    loadingOlderItems = false;
    if (!page || !standing() || request.asked !== selectedId) return;
    // The record is what the page went into; this is the paint that keeps the
    // reader's place as the history arrives above them.
    olderItemsAwaitingPaint = true;
    // The page is above the floor of what is drawn, so it is shown as the next
    // entries would have been from the cache.
    timelineSlice.showEarlier();
    await bindConversationCache().reread();
    if (standing()) paintChat({ olderItemsPrepended: true });
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
      <label class="rail-newagent-name">
        <span>Label <em>optional</em></span>
        <input type="text" data-new-agent-name maxlength="24" ${fieldTraits("line")}
          placeholder="Rail scroll" aria-label="What to call this agent" value="${esc(newAgentName())}">
      </label>
      <div class="rail-newagent-work">
        <label><span>Role <em>optional</em></span>
          <select data-new-agent-role aria-label="What this agent is to be">
            <option value="">No particular role</option>
            ${AGENT_ROLES.map((role) => `<option value="${esc(role.id)}"${role.id === newAgentWork().role ? " selected" : ""}>${esc(role.label)}</option>`).join("")}
          </select></label>
      </div>
      <p class="dim rail-newagent-resolved" data-new-agent-resolved></p>
    </div>`;
    body.dataset.newAgent = chosen;
    // Held outside the repaint: the picker rewrites itself when the harness
    // highlight moves, and a name half-typed must not go with it.
    body.querySelector("[data-new-agent-name]").oninput = (event) => {
      writeNewAgentName(event.target.value);
    };
    // The reader is told what "coding / deep" will actually start, before they
    // press: the device's grid answers it, and the answer is not guessable
    // from two words in a select.
    // What the chosen role will actually start, and how much direction it
    // will want — neither is guessable from a word in a select.
    const previewHost = body.querySelector("[data-new-agent-resolved]");
    const preview = () => {
      const role = newAgentWork().role;
      const chosen = role ? modelForRole(roleModelsOf(catalog), role) : null;
      if (!role) {
        previewHost.textContent = "";
        return;
      }
      if (!chosen) {
        previewHost.textContent = "No model is set for that role; the device default starts.";
        return;
      }
      const direction = AGENT_CAPABILITIES.find((entry) => entry.id === chosen.capability);
      previewHost.textContent = `Starts ${chosen.model} — ${direction?.direction || chosen.capability}`;
    };
    body.querySelector("[data-new-agent-role]").onchange = (event) => {
      writeNewAgentWork("role", event.target.value);
      preview();
    };
    preview();
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
      activityRuns?.dispose();
      activityRunsFor = runsFor;
      activityRuns = createActivityRuns({
        deviceId: cacheScope?.deviceId,
        entityId: identity.entityId,
        agentId: identity.agentId,
        call: (method, params) => chatRepository.currentCall()(method, params),
        onChange: () => {
          if (!standing() || activityRunsFor !== runsFor) return;
          // Presence alone is in the ordinary paint fingerprint; a later
          // announcement can replace the rows under the same run key, so the
          // record moving explicitly invalidates that fingerprint.
          paintedChat = null;
          paintChat();
        },
      });
      // Everything else the panel remembers about the conversation goes with
      // it: a line ruled in one thread marks nothing in the next, and how far
      // this panel read one says nothing about the other.
      leaveUnreadMarker();
      if (!unreadMarkers.has(runsFor)) unreadMarkers.set(runsFor, createUnreadMarker(() => {
        unreadFrom = null;
        if (standing()) paintChat();
      }));
      unreadMarker = unreadMarkers.get(runsFor);
      reportedRead = 0;
      reportedFloor = null;
      timelineSlice.reset();
    }
    return activityRuns;
  };

  // The marker's clock starts when the newest agent message has been read;
  // trailing user messages and activity do not delay it until scroll bottom.
  const unreadLineFor = (thread, readThrough = 0) => {
    const agent = agentInFocus();
    return unreadMarker?.update({
      cursor: agent?.read_through_sequence,
      unreadCount: agent?.unread_count || 0,
      items: threadItems(thread),
      readThrough,
    }) ?? null;
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
  /// touch to be the live tail. The record's cursor is that number: everything
  /// the conversation has said is in it or behind it.
  const lastSequenceOf = () => threadCache.readWindow()?.deliveredSequence || 0;

  /// How far the window has taken delivery of the conversation. The one number
  /// that moves for an item mutated in place — a call answered, a message
  /// marked seen — which changes what a row says without changing how many
  /// rows there are.
  const deliveredSequenceOf = () => lastSequenceOf();

  const threadItems = (thread) => (thread && thread.items) || [];

  const drawnChatContent = () => {
    const floor = timelineSlice.drawnFloor();
    const drawn = loadedConversationItems().filter((item) => {
      const sequence = Number(item.data?.sequence ?? NaN);
      return floor === null || !Number.isFinite(sequence) || sequence >= floor;
    });
    return JSON.stringify(drawn);
  };

  /** Where this rail is standing, for the links a message from another agent
   *  carries: the machine the conversation is held on, and the project whose
   *  page it is on — a workspace route is written from both, and the sender
   *  names only the workspace. */
  const conversationPlace = () => ({
    deviceId: context.deviceId ?? null,
    projectId: entity.projectId || context.projectId,
    // What this project is called, for the one row that names Build's own
    // agent for it: "Build agent" rather than four characters of an id (#40).
    projectName: knownProjectName(context) || projectName || "",
  });

  /** What this project's agents are called, for the one row that names an
   *  agent it is not standing on: a tracking notice's "X did Y". The maintainer
   *  asked for a name they recognise, and "Agent 01M2" is not one. */
  const conversationAgentLabels = () => {
    const place = conversationPlace();
    if (!place.projectId || !place.deviceId) return {};
    return agentLabelsOf(workspaceAgents(feedView, deviceKey(place.deviceId, place.projectId)));
  };

  /** What a reference written in a message points at (#56, wired in #63): the
   *  project's issues as this rail has already read them, its workspaces, and
   *  the agents standing in them. Nothing is fetched for this — a reference to
   *  something this client has not read stays the words the agent typed. */
  const conversationRefLinks = () => {
    const place = conversationPlace();
    if (!place.projectId || !place.deviceId) return null;
    const key = deviceKey(place.deviceId, place.projectId);
    return referenceLinks({
      place,
      issues: issuesBlock?.issues() || [],
      workspaces: (feedView?.workspaces || []).filter((workspace) => workspace.projectKey === key),
      agentGroups: workspaceAgents(feedView, key),
    });
  };

  /** What the resolver can answer for, as the paint sees it: how many issues
   *  have been read and how far they reach. In the fingerprint because a list
   *  landing after the paint turns prose into links, and nothing else on the
   *  row would have moved. */
  const refLinksSignature = () => {
    const issues = issuesBlock?.issues() || [];
    const workspaces = (feedView?.workspaces || []).filter((workspace) =>
      workspace.projectKey === deviceKey(conversationPlace().deviceId, conversationPlace().projectId));
    const ids = workspaces.map((workspace) => workspace.workspace_id || workspace.id).sort().join(",");
    return `${issues.length}:${issues.reduce((highest, issue) => Math.max(highest, issue.number || 0), 0)}:${ids}`;
  };

  /** The names as the paint sees them. In the fingerprint because a feed that
   *  renames an agent has to repaint a line already on screen, and out of it
   *  nothing else would notice. */
  const agentLabelsSignature = () =>
    Object.entries(conversationAgentLabels())
      .sort(([left], [right]) => (left < right ? -1 : 1))
      .map(([id, label]) => `${id}:${label}`)
      .join(",");

  const chatFingerprintOf = (thread, agentLabel) => {
    const openRuns = conversationRuns().openKeys();
    const threadState = controllerInFocus().threadState;
    return chatPaintFingerprint({
      deliveredSequence: deliveredSequenceOf(),
      itemCount: threadItems(thread).length,
      digests: digestsOf(thread),
      openRunKeys: openRuns,
      fetchedRunKeys: [...openRuns].filter((key) => fetchedRunItems(key)),
      selectedAgentId: selectedId,
      agentLabel,
      unreadFrom,
      detailLevel: detailLevel(),
      agentLabels: agentLabelsSignature(),
      refLinks: refLinksSignature(),
      slice: `${timelineSlice.signature()}:${threadCache.hasOlderItems()}`,
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
    // Keep the visit marker stable as daemon read cursors catch up.
    unreadFrom = unreadLineFor(thread);
    // Every open is a first paint: whatever the reader showed last time, the
    // panel opens on the newest entries, so the animation never carries more.
    const opening = !body.querySelector(".thread-items");
    if (opening) timelineSlice.reset();
    const fingerprint = chatFingerprintOf(thread, agentLabel);
    if (fingerprint === paintedChat && !opening) return;
    paintedDigests = digestsOf(thread);
    // The conversation an option reply is keyed under: the controller's, since
    // the record holds the window and not the name of the thread it is over.
    const conversationId = controllerInFocus().identity.conversationId || null;
    // The level reads the cached thread; it never changes what was cached.
    const held = threadItems(thread);
    const shown = itemsAtDetailLevel(held, detailLevel());
    const built = timelineEntries(shown, agentLabel, conversationId, paintedDigests, {
      openRuns: runs.openKeys(),
      runItemsOf: fetchedRunItems,
      threadState: controllerInFocus().threadState,
      unreadFrom,
      place: conversationPlace(),
      workspaces: (feedView?.workspaces || []).filter((workspace) =>
        workspace.projectKey === deviceKey(conversationPlace().deviceId, conversationPlace().projectId)),
      agentLabels: conversationAgentLabels(),
      refLinks: conversationRefLinks(),
      // So a timeline the level emptied says so, rather than claiming the
      // conversation has nothing on the record.
      hiddenByLevel: held.length - shown.length,
      slice: timelineSlice.request(),
      olderOnBridge: threadCache.hasOlderItems(),
    });
    timelineSlice.settle(built.sliced);
    paintedEntryKeyOf = built.entryKeyOf;
    // Taken after the slice settles, so the floor it settled on is not news to
    // the next tick.
    paintedChat = chatFingerprintOf(thread, agentLabel);
    paintedChatItemCount = held.length;
    paintedChatContent = drawnChatContent();
    // No composer in here: the box is pinned below this scroller, so what the
    // poll repaints is the timeline and only the timeline.
    timedPaint("chat", () => {
      paintThreadKeepingPlace(body, () => {
        paintThreadEntries(body, built);
        wireTimeline(body);
      }, { olderItemsPrepended });
      syncUserMessageTicks(body);
    });
  };

  const chatIsVisible = () => standing() && panelVisible && !document.hidden && shownPanelMode() === "chat";

  const paintChat = ({ olderItemsPrepended = false } = {}) => {
    if (!chatIsVisible()) return;
    const body = host.querySelector("#rail-body");
    if (!body) return;
    if (entity.chatCapable === false) {
      body.innerHTML = '<div class="rail-chat-loading">This workspace does not have an agent conversation yet.</div>';
      return;
    }
    if (conversationIsUnselected() || addingAgent()) {
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
    wireReaderMotion(body);
    body.onscroll = () => {
      if (!panelVisible) return;
      if (body.scrollTop <= OLDER_ITEMS_TRIGGER_PX) showEarlierEntries();
      scheduleUserMessageTickSync(body);
      reportRead(body);
    };
    syncComposer();
    syncSurfaces();
    syncUserMessageTicks(body);
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
      ? runDigestToFetch(digestsInHand(), runKey, lastSequenceOf(), runThrough)
      : null;
    await runs.whenFoldPainted();
    if (!digest) return;
    await runs.open(digest).catch((error) => {
      notifyError("Could not load this activity", error.message);
    });
  };

  const composerPlaceholder = () =>
    visibleAgents().length && !addingAgent() ? "Send a message to this agent…" : "Send a message to start an agent here…";

  /// The box you write in, pinned below the conversation instead of sitting at
  /// the end of it. It is a SIBLING of the scroller, so reading back through a
  /// long thread never takes the box off screen, and the growth of a box being
  /// typed into comes out of the thread above rather than pushing its own
  /// bottom edge past the panel.
  ///
  /// The footer carries what moves while an agent works — the Working clock,
  /// the git line, what it is observing, its context gauge — and the composer
  /// row inside it carries only the box, which nothing an agent does writes
  /// into. A phone's keyboard dropped swipes and taps while the rail rewrote
  /// things around the text (#138). The gauge reads out over the room the box
  /// keeps for it beside the paperclip.
  const composerRowHtml = () =>
    `<div class="rail-footer" id="rail-footer">
      ${railViewerHostHtml()}
      ${railObservationHostHtml()}
      ${railStatusRowHtml()}
      <div class="chat-recovery" id="rail-chat-recovery"></div>
      <div class="rail-composer" id="rail-composer">${composerHtml({
        inputId: COMPOSER_IDS.input,
        sendId: COMPOSER_IDS.send,
        hintId: COMPOSER_IDS.hint,
        placeholder: composerPlaceholder(),
        attachable: true,
        modelMenu: true,
        canInterrupt: agentCanInterrupt(agentOf(selectedId)),
      })}</div>
      ${composerGaugeHtml(COMPOSER_IDS.input)}
    </div>`;

  /// The one thing on the composer a poll can change: whether the send offers
  /// to stop the turn in flight, which moves every time an agent starts or
  /// finishes one. The box itself is never rebuilt for it — a rebuild would
  /// take the draft and the focus with it, mid-sentence. (The placeholder
  /// cannot move under a poll: it says whether there is an agent to talk to,
  /// and gaining one rebuilds the panel around a conversation.)
  /** The agent the panel is about — none while the chooser or the overview is
   *  up, whatever bubble is technically still selected behind it. */
  const agentInFocus = () => {
    if (selectedKind !== "agent") return null;
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
    syncContextGauge();
    if (!composerModelMenu) return;
    const agent = agentInFocus();
    const settled = composerController?.choice();
    const choice = composerDisplayChoice(agent, settled);
    composerModelMenu.set(catalog, choice.provider, choice, settled?.activeModel || "", settled?.activeEffort || "");
  };

  /// The share of its window the agent's last turn held, off the same record
  /// every push repaints the rail from. It writes its own node, so the box
  /// being typed into beside it is never touched.
  const syncContextGauge = () => composerGauge?.set(agentInFocus(), catalog);

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
      (path) => fetchThreadAttachment(
        chatRepository.currentCall(),
        controller.identity.entityId,
        path,
        bridgeCapabilities(context.deviceId)?.threads?.attachmentChunks === true,
      ),
      controller.threadState,
    );
    wireThreadRevisionLinks(body, (revisionId) =>
      revisionContents(controller.identity.entityId, revisionId, chatRepository.currentCall()));
    wireThreadLinks(body, openLink);
    wireThreadSentMessages(body, controller.threadState);
    wireThreadArrivals(body, controller.threadState);
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
      if (jumpToUserMessage(event, { reveal: drawDownTo })) return;
      if (event.target.closest?.(".thread-earlier")) {
        showEarlierEntries();
        return;
      }
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
      viewingContext: composerViewingContext,
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
    composerModelMenu = mountComposerModelMenu(panel, {
      ids: COMPOSER_IDS,
      onChoose: chooseModel,
      cacheKey: conversationKey(controller),
    });
    composerGauge = mountContextGauge(panel, { ids: COMPOSER_IDS });
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
    const runKey = runKeyHolding(body, sequence);
    if (!runKey || conversationRuns().isOpen(runKey)) return;
    await pressActivityRun(runKey);
  };

  /// The run a sequence is folded into: the one the held items say owns it,
  /// or, for a sequence they do not hold, the run whose cut half reaches over
  /// it. Nothing for a sequence that is a row of its own.
  const runKeyHolding = (body, sequence) => {
    const key = paintedEntryKeyOf(Number(sequence));
    if (key === null) return cutRunKeyAt(body, sequence);
    return activityRunDrawn(body, key) ? key : null;
  };

  const surfaceViewerCallbacks = () => ({
    modelLabel: surfaceModelLabel,
    onOpenThreadItem: async (sequence) => {
      const body = host.querySelector("#rail-body");
      drawDownTo(sequence);
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

  /** Everything the conversation's ⋮ offers, in sections (#124): whichever
   *  surfaces this agent has opened, then the conversation's two settings —
   *  how much of the thread to draw, and when it compacts. What the agent
   *  opened comes first because it is what a menu on a conversation is opened
   *  FOR; the settings sit under it as settings do, each under its own name,
   *  and the group of surfaces is absent when there are none. The levels are
   *  always there, which is what makes the menu itself always there — it used
   *  to vanish with the last surface, and the toggle has to be reachable from
   *  a conversation that has none. */
  const surfaceMenuGroupsInFocus = () => [
    surfaceMenuGroup(surfacesWithIssues(surfacesSeen().surfaces)),
    detailLevelMenuGroup(detailLevel()),
    compactionMenuGroupInFocus(),
  ].filter(Boolean);

  /** The compaction group, for a settled agent on a bridge that has the verb
   *  — none while the chooser is up, and none for an agent not yet born,
   *  which has no conversation to set. */
  const compactionMenuGroupInFocus = () => {
    const agent = settledAgentInFocus();
    if (!agent || !entity.entityId || !carriesCompactionSettings(context.deviceId)) return null;
    return compactionMenuGroup(agent);
  };

  const mountSurfaces = (panel) => {
    const pillHost = panel.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const viewerHost = panel.querySelector(`#${RAIL_VIEWER_ID}`);
    const observationHost = panel.querySelector(`#${RAIL_OBSERVATION_ID}`);
    if (!pillHost || !viewerHost || !observationHost) return;
    observationBlock = mountAgentObservation(observationHost);
    // The issues are a surface kind now (#34), so this supplies rows rather
    // than drawing a block of its own. It reads a different source from the
    // rest — the project's cached issue list, not the agent digest — which is
    // why it is mounted beside them and merged in at paint.
    issuesBlock = mountAgentIssues({
      deviceId: context.deviceId,
      projectId: context.projectId,
      onChanged: () => {
        syncSurfaces();
        paintSurfaceMenu();
      },
    });
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
    issuesBlock?.dispose();
    issuesBlock = null;
    observationBlock?.dispose();
    observationBlock = null;
    if (!surfacesBlock) return;
    surfacesBlock.dispose();
    surfacesBlock = null;
  };

  /** The agent digest's surfaces with this agent's issues merged in. Merged
   *  here rather than in the digest because they come from a different place
   *  and a different push: the project's cached issue list. An agent holding
   *  and tracking nothing adds nothing, so no pill appears. */
  const surfacesWithIssues = (surfaces) => {
    const entries = issuesBlock?.entriesFor(agentInFocus()?.id || null);
    if (!entries) return surfaces;
    return { ...(surfaces || {}), [ISSUES_ENTRY_KIND]: entries };
  };

  const syncSurfaces = () => {
    if (!surfacesBlock || !observationBlock) return;
    const seen = surfacesSeen();
    surfacesBlock.set(surfacesWithIssues(seen.surfaces), seen.at);
    // The observation panel reads the checklist out of the digest's own
    // payload; the issues are none of its business.
    observationBlock.set(seen.surfaces, {
      generation: seen.generation,
      working: agentInFocus()?.working === true,
    });
  };

  /** The menu is kept within the panel: on a phone the panel stops on the
   *  bubble strip, which is drawn over it and would cover the menu's tail. */
  const paintSurfaceMenu = () => {
    if (!standing()) return;
    const region = host.querySelector(SURFACE_MENU_SELECTOR);
    if (!region) return;
    closeSurfaceMenu = mountMenuIfChanged(region, surfaceMenuHtml(surfaceMenuGroupsInFocus()), {
      onChoose: chooseFromSurfaceMenu,
      keepWithin: () => host.querySelector("#rail-panel"),
    });
  };

  /** One menu, three kinds of choice: a level is a way of reading what is
   *  already here, a compaction row is when the conversation compacts, a
   *  surface kind opens what the agent made. */
  const chooseFromSurfaceMenu = (optionId) => {
    const level = detailLevelOfOptionId(optionId);
    const compaction = compactionLimitOfOptionId(optionId);
    if (level) chooseDetailLevel(level);
    else if (compaction) chooseCompaction(compaction.maxContextTokens);
    else openSurfaceOverlayForKind(optionId);
  };

  const chooseCompaction = (maxContextTokens) => {
    const agent = settledAgentInFocus();
    if (!agent || !entity.entityId) return;
    void compactionChoice.choose({ entityId: entity.entityId, agent, maxContextTokens });
  };

  /** Read this conversation at a different level: remembered for it alone, and
   *  the timeline redrawn from the same cached items.
   *
   *  The menu's own tick is repainted after the press has finished shutting it
   *  — remounting the menu mid-animation would swap the closing box for a fresh
   *  one and lose the movement. */
  const chooseDetailLevel = (level) => {
    const id = conversationDetailId();
    if (chosenDetailLevels.get(id) === level) return;
    chosenDetailLevels.set(id, level);
    void detailRecord(id).write({ level });
    motionSettled().then(paintSurfaceMenu);
  };

  const openSurfaceOverlayForKind = (kind) => {
    closeSurfaceOverlay();
    surfaceOverlay = openSurfaceOverlay(kind, {
      ...surfaceViewerCallbacks(),
      cacheKey: key,
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

  /** A reference in the conversation goes where it points, as far as the
   *  work-item surfaces can take it. */
  const openLink = (link) => {
    if (link.issue_id || link.plan_id) go({ name: "issue", projectId: entity.projectId, id: link.issue_id || link.plan_id });
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
    // Shown is not read: a window left showing a chat overnight must not mark
    // what agents post, nor keep the user's session alive. What is on screen
    // is reported when the user comes back (readerPresence.js).
    if (!chatIsVisible() || !readerIsHere()) return;
    const agent = agentInFocus();
    const thread = threadFor();
    // The drawn rows are all `readThroughSequence` can ask, so the items this
    // level left out are folded back in: a reader who has scrolled past where a
    // hidden item would have been has read it, and without this the cursor
    // would stall under a conversation whose tail is activity.
    //
    // One reading serves both the marker and the report: they are the same
    // question. The marker still anchors on agent MESSAGES alone, and the walk
    // stops at the first drawn row the reader has not reached, so nothing a
    // level hides can carry the line past something they have yet to read.
    const read = readThroughHiddenItems(readThroughSequence(body), threadItems(thread), detailLevel());
    unreadFrom = unreadLineFor(thread, read);
    if (!agent || !agent.unread_count) return;
    const controller = controllerForAgent(agent);
    if (!controller.identity.entityId || !controller.identity.agentId) return;
    // The floor is where what the reader was shown starts: the top of the drawn
    // slice when it cuts the window, else the window's own.
    const floor = timelineSlice.drawnFloor() ?? threadCache.windowFloorSequence();
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
    // The overview has no composer; the draft waits for the next `+`.
    if (!panel || selectedKind === "overview") return;
    panel.dataset.body = "";
    paintPanel();
  };

  const renameAgentIdentity = (fromAgentId, toAgentId, controller) => {
    if (!fromAgentId || !toAgentId || fromAgentId === toAgentId) return;
    const renamedAgentIsSelected = selectedId === fromAgentId;
    if (threadAgentId === fromAgentId) threadAgentId = toAgentId;
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
    const kindBeforeCreate = selectedKind;
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
    selectKind("agent");
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
      await postSubmission(controller, addressedSubmission, (posted) =>
        rekeyPostedMessage(handle, provisionalMessageKey, provisionalMessage, posted, addressedSubmission));
      messageDelivered = true;
      await wakeAgent(addressedSubmission);
    };

    // The agent that failed to exist leaves the id under the selection, and
    // takes the panel back only where the panel still shows it. A reader who
    // selected something else meanwhile keeps it — the overview included, which
    // kept the provisional id underneath (#148). Says whether it was shown.
    const leaveProvisional = () => {
      const shown = selectedKind === "agent" && selectedId === provisionalAgentId;
      if (selectedId === provisionalAgentId) chooseAgent(selectedBeforeCreate);
      return shown;
    };
    const onRevert = (error) => {
      if (operationIsUncertain(error)) {
        controller.recordOperationFailure(submission, error);
        if (leaveProvisional()) {
          selectKind("add");
          paint();
        }
        return;
      }
      if (leaveProvisional()) {
        selectKind(kindBeforeCreate);
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

  /** Send a message to an agent that exists.
   *
   *  The message goes into the conversation's own record before the post
   *  leaves, and the record is what every reader of that conversation paints
   *  from — so the announcement that write makes is what puts it on screen,
   *  here and in every other tab and surface on it alike. One cache round trip,
   *  not the frame send was pressed in: there is one conversation, and this is
   *  where it is kept rather than in a second store beside the panel. What the
   *  post answers stamps the sequence onto it; a refusal takes it back out. */
  const deliverSubmission = (controller, submission) => {
    const address = conversationRecordAddress(cacheScope, submission.address);
    const agent = agentOf(submission.address.agentId);
    const wakesAgent = (entity.kind === "branch" || entity.kind === "workspace") && !agentIsUp(agent);

    let messageDelivered = false;
    if (address) void writeProvisionalMessage(address, submission.operationId, submission.message);

    const call = async () => {
      await postSubmission(controller, submission, (posted) => {
        if (address) void settleProvisional(address, submission, posted);
      });
      messageDelivered = true;
      if (wakesAgent) {
        await wakeAgent(submission);
      }
    };

    runOptimistic({
      scope: pendingAgentsScope(),
      records: wakesAgent && agent ? [startingRecord(agent.id)] : [],
      call,
      failureSummary: "Message failed",
      onRevert: (error) => {
        if (messageDelivered) return;
        if (address) void withdrawProvisionalMessage(address, submission.operationId);
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

  /** Where the user was standing when they wrote this: the workspace this rail
   *  reached the project's conversation from, ahead of whatever they attached
   *  (core/chatRepository.js merges the two). Every other rail sends the
   *  message as it is — a workspace's own agents are already in it, and the
   *  project's own page stands in none. */
  const stamped = (message) =>
    fromWorkspace ? { ...message, viewing_context: { version: 1, items: [fromWorkspace] } } : message;

  /** A typed message. `interrupt` rides on the post rather than travelling as a
   *  verb of its own: Build never stops a turn without one to put in its place,
   *  and a second round trip is a window in which the agent starts a fresh turn
   *  or finishes. One send path, one flag. */
  const sendFrom = (controller, body, attachments, { interrupt = false } = {}) => {
    const message = stamped({ body, attachments, ...(interrupt ? { interrupt: true } : {}) });
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
    const message = stamped({ option_reply: { message_id: messageId, option_ids: optionIds } });
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
    // The `+` below the line adds an agent to the WORK ITEM, so pressing it
    // from the project's conversation stands the rail back there first and
    // opens the chooser — the same place pressing it below lands.
    if (onProjectAgentRail && type === "add") return () => swap.toWorkItem(null, { adding: true });
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
      if (standing() && entityId) swap.toProject(entityId);
    } catch (error) {
      mintingProjectAgent = false;
      if (standing()) notifyError("No conversation for this project", error.message || String(error));
    }
  };

  /** What pressing a bubble selects. The ghost, and the project's own bubble
   *  on the project's rail, stand for the conversation the rail is on. */
  const pressedSelection = (type, agentId) => ({
    kind: type === "add" || type === "overview" ? type : "agent",
    agentId: type === "agent" ? agentId : selectedId,
  });

  const isSelected = ({ kind, agentId }) =>
    kind === selectedKind && (kind !== "agent" || agentId === selectedId);

  /** Every bubble toggles the same way (#148): pressing the selected one puts
   *  the panel away, or brings it back on the same selection, whichever way it
   *  is on the screen; pressing another selects it and shows the panel. */
  const pressBubble = (type, agentId) => {
    const swapping = swapForPress(type, agentId);
    if (swapping) {
      swapping();
      return;
    }
    const pressed = pressedSelection(type, agentId);
    if (!isSelected(pressed)) select(pressed);
    else if (panelOut()) closePanel({ restoreFocus: true });
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
    selectKind("agent"); // opening a real conversation ends the chooser
    selectionFromMemory = false;
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
  const openNewAgent = () => {
    if (!entity.entityId) return;
    selectKind("add");
    // The browser's stored harness preference seeds the chooser's highlight,
    // clamped to the offer — the record the silent + used to spend outright.
    // The human now sees the choice before anything is created; the send is
    // what creates, exactly as it does on a branch with no agents at all.
    seedNewAgentDefaults();
    showPanel();
    disposeTui();
    paint();
  };

  /** The chat overview, in the panel every conversation is shown in. */
  const openOverview = () => {
    selectKind("overview");
    showPanel();
    paint();
  };

  const selectors = {
    agent: ({ agentId }) => {
      openAgent(agentId);
      // The other conversation is on disk already: the paint opens its record
      // and draws it, with no round trip between the press and the words.
      paint();
    },
    add: openNewAgent,
    overview: openOverview,
  };
  const select = (pressed) => selectors[pressed.kind](pressed);

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
    if (!standing() || entity.entityId !== entityId || settledAgentInFocus()?.id !== agent.id) return;
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

  const unsubscribeFeed = subscribeFeed((feed) => {
    // One machine's rows, not the merge: this work item is on the machine its
    // link named, and every machine mints a `proj-1` — so the row is looked for
    // by the machine and the project together.
    feedView = deviceFeedView(feed, context.deviceId);
    feedRow = toolbarIdentity(feedRoute(), feedView).row;
    paintRailStatus();
    // A tracking notice names the agent that acted, and that name comes off
    // the feed — so a feed that moved can change a line already on screen.
    // The fingerprint decides whether it actually repaints.
    paintChat();
  });

  // The feed names this work item — agents included — and its snapshot replays
  // synchronously at subscribe, so the strip and the panel stand up on it in
  // the same frame the rail is mounted in. The row record, read a turn later,
  // is the same shape from a page the pushes keep more current; it reconciles.
  const feedSeedEntity = feedRow ? railEntity(feedRow, context.kind) : null;
  if (feedSeedEntity && feedSeedEntity.agents.length && !visibleAgents().length) {
    reconcileOptimistic(pendingAgentsScope(), feedSeedEntity.agents, { keyOf: agentIdOf });
    entity = feedSeedEntity;
    // The same selection a read makes, so the seed and the record agree on
    // whose conversation the panel is showing. Only a row that names its agents
    // seeds: an agentless row has no selection to make, and making one anyway
    // would wipe the remembered choice the record is about to honor.
    settleSelection();
  }
  paint();
  refresh();
  readProjectAgent();
  readProjectPageName();
  // The harnesses and their models, asked of the machine this rail is mounted
  // on and held there (core/modelCatalog.js). The new-agent view leads with
  // that bridge's default, which is this answer's to give, so a paint that
  // lands before it holds the client's own first harness and moves when the
  // answer does.
  // A machine that has not answered yet offers what it last offered, off disk,
  // and its own answer replaces that when it lands: the rail follows it, and
  // one heard before the first read settled is the one that stands.
  let catalogSettled = false;
  let catalogHeard = null;
  const takeCatalog = (offered) => {
    catalog = offered;
    seedNewAgentDefaults();
    paint();
  };
  const stopFollowingCatalog = followDeviceCatalog(context.deviceId, (held) => {
    if (!standing() || !held) return;
    if (catalogSettled) takeCatalog(held);
    else catalogHeard = held;
  });
  Promise.all([deviceCatalog(context.deviceId), readProjectAgentSetting()]).then(([offered, setting]) => {
    if (!standing()) return;
    projectAgentSetting = setting;
    catalogSettled = true;
    takeCatalog(catalogHeard || offered);
  });
  // The elapsed-time clock: the one timer left on the rail, and it says nothing
  // about the wire — it is the "working for 4m" line counting.
  statusTicker = setInterval(paintRailStatus, 1000);
  // A harness out of usage on this machine (#58): a strip at the top of the
  // conversation, counting down to its reset, whichever agent is open.
  usageLimitBanner = mountUsageLimitBanner(() => host.querySelector(`#rail-panel:not([data-body="${OVERVIEW_BODY}"])`), context.deviceId);
  const visibilityChanged = () => {
    if (document.hidden) leaveUnreadMarker();
    else paintChat();
  };
  document.addEventListener("visibilitychange", visibilityChanged);
  const stopWaitingForReader = onReaderReturns(paintChat);
  document.addEventListener("keydown", dismissOnEscape);
  document.addEventListener("pointerdown", dismissOnOutsidePointer);
  window.addEventListener("hashchange", dismissPopover);
  const cancelPanelMotion = () => panelMotion.cancel();
  panelMotion = createChatPanelMotion(host, { onPhase: syncPopover });
  window.addEventListener("resize", cancelPanelMotion);

  /// Everything the rail hears — the cache, the feed, the machine's catalog,
  /// the composer's controller, the clock and the reader coming back — stopped
  /// once: when the rail is disposed, or before that, the moment the machine it
  /// stands over is retired and its repository with it. A feed or a cache
  /// write delivered during the retirement then finds nobody to paint through
  /// the retired scope, and the rail keeps its last paint until the shell
  /// stands the route's rail up again (core/shell.js).
  let listening = true;
  const stopListening = () => {
    if (!listening) return;
    listening = false;
    overview.close();
    activityRuns?.dispose();
    activityRuns = null;
    for (const record of detailRecords.values()) record.dispose();
    pinnedRecord.dispose({ flushPending: false });
    overviewScopeRecord.dispose({ flushPending: false });
    document.removeEventListener("visibilitychange", visibilityChanged);
    stopWaitingForReader();
    unwatchCache();
    clearInterval(statusTicker);
    statusTicker = null;
    stopFollowingCatalog();
    unsubscribePending();
    unsubscribeFeed();
    unsubscribeComposerController?.();
    unsubscribeComposerController = null;
  };
  const stopHearingRetirement = chatRepository.onRetired(stopListening);

  return {
    /** Put the panel away without touching the docked/card preference — what a
     *  toolbar tab press means on a phone, where the chat covers the page it
     *  is about to change (#62). Unlike `dismissPopover` this does not spare a
     *  PINNED panel: the pin is remembered across widths, so a reader who
     *  docked the chat on a desktop would otherwise never get it out of the
     *  way on a phone. The preference itself is left exactly as it was. */
    collapse() {
      closePanel();
    },
    dispose() {
      disposed = true;
      stopHearingRetirement();
      stopListening();
      for (const marker of unreadMarkers.values()) marker.leave();
      panelMotion.cancel();
      usageLimitBanner.dispose();
      disposeTitleMotion();
      disposeTui();
      disposeSurfaces();
      completionToast.dispose();
      completionTracker.reset();
      disposeComposerClearance?.();
      disposeComposerClearance = null;
      closeSurfaceMenu?.();
      composerControl?.dispose?.();
      disposeComposerModelMenu();
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
