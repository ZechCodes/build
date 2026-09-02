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
  agentCanInterrupt,
  agentHasTerminal,
  agentTitle,
  canRemoveAgent,
  providerLabel,
  railBubbles,
  railEntity,
  railWorkStatus,
  removeAgentConfirm,
  selectAgentId,
} from "./agentRailModel.js";
import { createAgentSelection } from "./agentSelection.js";
import { NO_AGENT_CHOICE, chosenProviderId, reconcileAgentChoice } from "./agentChoice.js";
import { confirmAction } from "./confirm.js";
import {
  insertRecord,
  isPending,
  isProvisionalKey,
  patchRecord,
  projectOptimistic,
  projectPending,
  provisionalKey,
  reconcileOptimistic,
  removeRecord,
  runOptimistic,
  subscribeOptimistic,
} from "./optimistic.js";
import { EXITING_ATTRIBUTE, patchList, rekeyEntry } from "./patchList.js";
import { hide, reveal } from "./motion.js";
import { composerHtml, mountComposerModelMenu } from "./composer.js";
import { catalogForProvider, creatableCatalog, modelParams, providerCardsHtml } from "./modelPicker.js";
import { markSeen } from "./inboxView.js";
import { notifyError } from "./notify.js";
import { cacheDeviceId } from "./cacheScope.js";
import { entityIdOf } from "./entityId.js";
import { readCached, writeCached } from "./localCache.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  MUTATION_THREAD_PAGE,
  createThreadCache,
  paintThreadKeepingPlace,
  revealThreadSequence,
  threadHtml,
  wireThreadAttachments,
  wireThreadOptions,
  wireThreadComposer,
  threadItemKey,
  wireThreadLinks,
  wireThreadRevisionLinks,
  writeThreadKeepingComposer,
} from "./thread.js";
import { mountAgentSurfaces, openSurfaceOverlay } from "./agentSurfaces.js";
import { surfaceMenuOptions } from "./agentSurfacesModel.js";
import { menuButtonMarkup, mountMenuIfChanged } from "./splitButton.js";
import { mountAgentTab } from "./surfaceTabs.js";
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
/** A pill that is in the row rather than on its way out of it. */
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
const drafts = new Map(); // `${entityId}:${agentId}` → { body, attachments }
const chosenAgent = new Map(); // entity key → agent id the human last opened
// What the next agent on a work item will be created as — the harness card the
// human pressed and the model menu's selection, held until there is an agent to
// write them to. Nothing goes to the bridge until then: there is no entity
// choice worth writing for a checkout that may never be adopted.
const newAgentChoices = new Map(); // entity key → { provider, model, effort }
// Chat or TUI, per work item: the terminal is the basement, so walking into a
// different branch or issue starts you in the conversation whatever face of the
// last one you were looking at.
const panelModes = new Map();

/** Forget what the rail remembers. For tests, and for a session teardown — the
 *  drafts and choices belong to the person who was signed in. */
export function resetAgentRailMemory() {
  drafts.clear();
  chosenAgent.clear();
  panelModes.clear();
  newAgentChoices.clear();
}

const railKey = (context) =>
  context.kind === "issue" ? `issue:${context.issueId}` : `branch:${context.projectId}:${context.branch}`;

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

const WORKING_SHAPE = "working";
const STARTING_SHAPE = "starting";
const QUIET_SHAPE = "quiet";

const RAIL_STATUS_LEAD_CLASS = {
  [WORKING_SHAPE]: "rail-status-lead rail-status-working",
  [STARTING_SHAPE]: "rail-status-lead rail-status-starting",
  [QUIET_SHAPE]: "rail-status-lead",
};

/** Pure: which of the two things the row's lead is saying — how long the turn
 *  in flight has been running, or when the session began — or that it has
 *  nothing to say. */
export function railStatusShape(status) {
  if (status.working) return WORKING_SHAPE;
  if (status.starting) return STARTING_SHAPE;
  return QUIET_SHAPE;
}

/// Pure: the markup each shape of the lead is made of, written once when the
/// shape changes and never again — the clock inside it is set as text.
///
/// "Working" is a span of its own because it is the one word the row gives up
/// when the pills want the room, and a tick that rebuilt it would take the
/// collapse with it.
export function railStatusLeadHtml(shape) {
  if (shape === WORKING_SHAPE) {
    return `<span class="rail-status-working-word">Working</span> <span class="rail-status-text"></span>`;
  }
  if (shape === STARTING_SHAPE) return `<span class="rail-status-text"></span>`;
  return "";
}

/** Pure: how far the work item stands from upstream and its diffstat, in one
 *  group pinned to the row's end so the ticking clock widens into the pills'
 *  room rather than shoving the facts along. */
export function railStatusGitHtml(status) {
  const sync = status.sync ? `<span class="rail-status-sync mono">${esc(status.sync)}</span>` : "";
  const stat = status.stat ? `<span class="rail-status-stat mono">${esc(status.stat)}</span>` : "";
  return sync + stat;
}

function paintStatusLead(lead, status) {
  const shape = railStatusShape(status);
  if (lead.dataset.shape !== shape) {
    lead.dataset.shape = shape;
    lead.className = RAIL_STATUS_LEAD_CLASS[shape];
    lead.innerHTML = railStatusLeadHtml(shape);
  }
  lead.hidden = shape === QUIET_SHAPE;
  const text = lead.querySelector(STATUS_TEXT_SELECTOR);
  if (text) text.textContent = status.working || status.starting;
}

function paintStatusGit(git, status) {
  const html = railStatusGitHtml(status);
  if (git.innerHTML !== html) git.innerHTML = html;
  git.hidden = !html;
}

/// The one row pinned above the composer: what the turn is doing, the pills the
/// agent's surfaces put up, and the git facts — in that order, the pills
/// scrolling in whatever room the other two leave them.
const railStatusRowHtml = () =>
  `<div class="rail-status" id="${RAIL_STATUS_ID}" hidden>
    <span class="rail-status-lead" id="${RAIL_STATUS_LEAD_ID}" hidden></span>
    <div class="rail-status-pills" id="${RAIL_STATUS_PILLS_ID}" role="group" aria-label="Agent surfaces"></div>
    <span class="rail-status-git" id="${RAIL_STATUS_GIT_ID}" hidden></span>
  </div>`;

/// Where a pill's viewer opens: the last thing in the conversation column,
/// above the line that tops the composer block, so it pushes the conversation
/// up as it grows rather than covering it.
const railViewerHostHtml = () => `<div class="rail-surfaces-viewer" id="${RAIL_VIEWER_ID}" hidden></div>`;

function surfaceMenuHtml(options) {
  return options.length ? menuButtonMarkup(SURFACE_MENU_LABEL, options, { title: SURFACE_MENU_TITLE, icon: true }) : "";
}

function surfaceMenuRegionHtml(options) {
  return `<span class="${SURFACE_MENU_CLASS}">${surfaceMenuHtml(options)}</span>`;
}

export function panelHeadHtml(who, mode, { removable = false, hasTerminal = true, surfaceOptions = [] } = {}) {
  const removeTitle = `Remove ${who} from this branch`;
  const remove = removable
    ? `<button type="button" class="iconbtn rail-remove" title="${esc(removeTitle)}"
        aria-label="${esc(removeTitle)}">−</button>`
    : "";
  const showingTui = mode === "tui";
  const tuiTitle = showingTui ? "Back to the conversation" : "Show the terminal";
  const tui = hasTerminal
    ? `<button type="button" class="rail-mode rail-tui${showingTui ? " on" : ""}"
        aria-pressed="${showingTui}" title="${tuiTitle}">TUI</button>`
    : "";
  return `<div class="rail-head">
    <span class="rail-who">${esc(who)}</span>
    ${tui}
    ${surfaceMenuRegionHtml(surfaceOptions)}
    ${remove}<button type="button" class="iconbtn rail-collapse" title="Collapse the conversation"
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
  const key = railKey(context);
  const selection = context.selection || createAgentSelection();
  let entity = railEntity(null, context.kind);
  let selectedId = chosenAgent.get(key) || null;
  selection.set(selectedId);
  // A collapsed rail has no composer to focus at all — the human just cut
  // this branch and is about to type into it, so that intent outranks
  // whatever they left the rail at on the last one.
  let expanded = context.autofocusComposer === true || readExpanded();
  let mode = panelModes.get(key) || "chat";
  let poll = null;
  let disposed = false;
  let tui = null; // the mounted PTY pane, in TUI mode
  let threadCache = createThreadCache();
  // Choosing the next agent's harness, with the chooser in the panel. Entered
  // by the strip's `+`, left by the send that creates the agent or by opening
  // any existing bubble.
  let addingAgent = false;
  let threadAgentId = null; // whose conversation the cache holds
  let loadingOlderItems = false; // a page of history is in flight
  let threadSeedTried = false; // one cache seed per conversation key
  let lastPersistedSequence = 0; // the window already on disk, to skip idle rewrites

  /** The local cache's address for the open conversation, or null while the
   *  entity is not yet known (no feed row) or no session device is live. */
  const threadCacheAddress = () => {
    const deviceId = cacheDeviceId();
    const entityId = context.kind === "issue" ? context.issueId : feedRow ? entityIdOf(feedRow) : null;
    if (!deviceId || !entityId) return null;
    return { deviceId, entityId, kind: "thread", sub: selectedId || "" };
  };

  /** Seed the empty cache from the saved window, once per conversation key.
   *  After a seed the next detail read is a forward delta rather than a first
   *  page — the history is already local. A live window refuses the seed.
   *
   *  A successful seed claims the cache for the agent it was read for
   *  (threadAgentId): the rail remembers which bubble was open across
   *  remounts, so without the claim the first threadFor of a revisit reads
   *  "different agent" and wipes the window this seed just opened — after its
   *  delta cursor was already sent, which is a conversation that paints empty
   *  until the safety poll. And it paints: the reader is owed the history in
   *  hand, not a loading frame until the wire answers. */
  const trySeedThread = async () => {
    const address = threadCacheAddress();
    if (!address || threadSeedTried) return;
    threadSeedTried = true;
    const seededFor = selectedId;
    const record = await readCached(address);
    if (disposed || !record || seededFor !== selectedId) return;
    if (threadCache.seedWindow(record.value)) {
      lastPersistedSequence = record.value.deliveredSequence || 0;
      threadAgentId = seededFor;
      paintChat();
    }
  };

  /** Persist the window when it has moved. Fire-and-forget, sequence-guarded:
   *  a repaint that absorbed nothing new writes nothing. */
  const persistThreadWindow = () => {
    const address = threadCacheAddress();
    const window = threadCache.readWindow();
    if (!address || !window || window.deliveredSequence === lastPersistedSequence) return;
    lastPersistedSequence = window.deliveredSequence;
    writeCached(address, window);
  };

  /** Drop the conversation cache — and with it, the seed's one-shot flag, so
   *  the next conversation key seeds from its own saved window. */
  const resetThreadCache = () => {
    threadCache.reset();
    threadSeedTried = false;
    lastPersistedSequence = 0;
    absorbedThreadPayload = null;
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
  let surfacesBlock = null;
  let surfaceOverlay = null; // the surface a menu option opened, over the panel
  let closeSurfaceMenu = null; // shuts the head's ⋯, and with it its outside-press watch


  const agentIdOf = (agent) => agent.id;

  const openConversation = (agentId) => {
    chooseAgent(agentId);
    resetThreadCache();
    threadAgentId = agentId;
  };
  const pendingAgentsScope = () => `agents:${key}`;
  const pendingThreadScope = (agentId) => `thread:${key}:${agentId || ""}`;
  const visibleAgents = () => projectOptimistic(pendingAgentsScope(), entity.agents, { keyOf: agentIdOf });

  const agentOf = (id) => visibleAgents().find((agent) => agent.id === id) || null;
  /** Open this agent's conversation, and tell everything else on screen: the
   *  bubble strip is the selector for the whole work item, not just the rail. */
  const chooseAgent = (id) => {
    selectedId = id || null;
    if (selectedId) chosenAgent.set(key, selectedId);
    else chosenAgent.delete(key);
    if (!isProvisionalKey(selectedId)) selection.set(selectedId);
  };
  const conversationKey = () => `${entity.entityId || key}:${selectedId || AGENT_NOT_YET_BORN}`;
  const draftOf = () => drafts.get(conversationKey()) || { body: "", attachments: [] };
  const writeDraft = (next) => drafts.set(conversationKey(), { ...draftOf(), ...next });

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
    const said = newAgentChoices.get(key) || NO_AGENT_CHOICE;
    return { ...said, provider: chosenProviderId(creatable(), said) };
  };
  const writeNewAgentChoice = (next) => newAgentChoices.set(key, next);

  /** That choice as `agent.add` params: empties omitted, so the harness's own
   *  default stands where nothing was said. */
  const newAgentParams = () => {
    const choice = newAgentChoice();
    const { models } = catalogForProvider(catalog || {}, choice.provider);
    return modelParams(models || [], choice.model, choice.effort, choice.provider);
  };

  /** The adopting caller for a checkout Build owns nothing in.
   *
   *  A view whose other surfaces can adopt too owns the adopter and hands it
   *  down (`context.adopting`), so the rail and the Changes review claim the
   *  checkout once between them. Standing alone, the rail makes its own once
   *  the payload says which checkout it is, and keeps it — it holds the run it
   *  mints. */
  const adoptingCall = () => {
    if (context.adopting) return context.adopting() || null;
    if (!adopting && entity.adoptable && entity.projectId) {
      const call = (method, params) => App.call(method, params);
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
  const feedRoute = () =>
    context.kind === "issue"
      ? { name: "issue", projectId: context.projectId, id: context.issueId }
      : { name: "branch", projectId: context.projectId, branch: context.branch };

  const loadedConversationItems = () => {
    const window = threadCache.readWindow();
    return (window && window.items) || [];
  };

  /// What the row says about itself once the lead, the pills and the git facts
  /// have each had their say: whether there is anything in it to show, and
  /// whether the word "Working" still has room to stand in.
  ///
  /// A pill on its way out has already given the word its room back, so the two
  /// cross rather than queue.
  const syncRailStatusRow = () => {
    const row = host.querySelector(`#${RAIL_STATUS_ID}`);
    if (!row) return;
    const lead = row.querySelector(`#${RAIL_STATUS_LEAD_ID}`);
    const pills = row.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const git = row.querySelector(`#${RAIL_STATUS_GIT_ID}`);
    row.hidden = lead.hidden && git.hidden && !pills.children.length;
    const word = lead.querySelector(WORKING_WORD_SELECTOR);
    if (!word) return;
    if (pills.querySelector(STANDING_PILL_SELECTOR)) hide(word, { axis: "width" });
    else reveal(word, { axis: "width" });
  };

  const paintRailStatus = () => {
    const row = host.querySelector(`#${RAIL_STATUS_ID}`);
    if (!row) return;
    const openAgentLabel = providerLabel((agentOf(selectedId) || {}).provider);
    const status = railWorkStatus(feedRow, Date.now(), loadedConversationItems(), openAgentLabel);
    paintStatusLead(row.querySelector(`#${RAIL_STATUS_LEAD_ID}`), status);
    paintStatusGit(row.querySelector(`#${RAIL_STATUS_GIT_ID}`), status);
    syncRailStatusRow();
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
    // The saved window first, so a revisit's first read is a forward delta
    // with the history already local. One try per conversation key.
    await trySeedThread();
    const askedAgentId = selectedId && !isProvisionalKey(selectedId) ? { agent_id: selectedId } : {};
    const scope = { ...threadCache.cursorParam(), ...askedAgentId };
    if (context.kind === "issue") {
      return App.call("issue.get", { issue_id: context.issueId, ...scope });
    }
    return App.call("branch.get", { project_id: context.projectId, branch: context.branch, ...scope });
  };

  const refresh = async () => {
    const asked = selectedId;
    let payload;
    try {
      payload = await detail();
    } catch (error) {
      // The agent we asked about is not on this work item any more — its run
      // was replaced, or it was retired. The daemon refuses rather than
      // answering with somebody else's conversation, so let the choice go and
      // the next tick reopens on whichever agent is here now. Without this the
      // rail would ask the same refused question forever.
      if (asked && !isProvisionalKey(asked) && /agent_id/.test((error && error.message) || "")) {
        openConversation(null);
      }
      // Anything else — a branch that stopped resolving (finished, renamed) —
      // leaves the rail as it was rather than blanking the conversation under
      // the reader.
      return;
    }
    if (disposed) return;
    // The human opened a different bubble while this read was in flight: it
    // answers about the conversation they just left, and folding its delta into
    // the cache the switch just cleared would show one agent's words under
    // another's name. Drop it; the next tick asks about the right one.
    if (asked !== selectedId) return;
    const answered = railEntity(payload, context.kind);
    // A branch is read off whichever source knows most about it, and the only
    // source that knows about agents is the run behind it. A tick that cannot
    // resolve the run answers off the bare checkout instead — no run, no
    // conversation, no agents — and the next tick has all three back. Believing
    // the first of those closes the conversation that is open: the strip drops
    // to a ghost, the head renames itself, and the panel is rebuilt around a
    // NEW textarea, which takes the words, the caret and, on a phone, the
    // keyboard with them. At a poll every 1.6 seconds that is a message that
    // cannot be typed at all.
    //
    // So an answer that loses the agents has to say it twice. A run that is
    // really gone (finished, abandoned) keeps saying it and the rail falls back
    // to the ghost as it always did, one tick later; a hiccup says it once and
    // is dropped.
    if (!answered.agents.length && visibleAgents().length && !agentlessOnce) {
      agentlessOnce = true;
      return;
    }
    agentlessOnce = false;
    entity = answered;
    reconcileOptimistic(pendingAgentsScope(), answered.agents, { keyOf: agentIdOf });
    chooseAgent(selectAgentId(visibleAgents(), selectedId));
    // Whose conversation this payload carries: the agent we asked about, or —
    // when we asked about none, which is every first read — the entity's own,
    // which is the agent the selection just landed on (its first).
    if (!isProvisionalKey(selectedId)) threadOwner = asked === null ? selectedId : asked;
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
    paintStrip(strip, railBubbles({ agents: visibleAgents(), selectedId, kind: entity.kind }));
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

  const shownPanelMode = () => (agentHasTerminal(agentInFocus()) ? mode : "chat");

  const wantedPanelBody = () => `${shownPanelMode()}:${addingAgent ? "new" : selectedId || AGENT_NOT_YET_BORN}`;

  const adoptPanelBody = () => {
    const panel = host.querySelector("#rail-panel");
    if (panel) panel.dataset.body = wantedPanelBody();
  };

  const paintPanel = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    const agent = agentInFocus();
    const who = agent ? agentTitle(agent) : entity.kind === "issue" ? "Issue agent" : "New agent";
    const settled = settledAgentInFocus();
    const removable = canRemoveAgent({
      agents: visibleAgents(),
      agentId: settled ? settled.id : null,
      kind: entity.kind,
    });
    const hasTerminal = agentHasTerminal(agent);
    // Which face this agent can actually wear. `mode` is remembered per work
    // item, so opening a terminal-less agent's bubble — or one whose digest
    // stopped offering a terminal under an open panel — arrives holding "tui"
    // for a screen that does not exist. The remembered choice is kept rather
    // than rewritten, so the agent beside it that does have a terminal is still
    // where the human left it.
    const shownMode = shownPanelMode();
    // The head is rewritten only when what it SAYS changed: the name, whether
    // this agent can be taken back off, and whether it has a basement.
    const wantedHead = `${who}:${removable ? "removable" : "kept"}:${hasTerminal ? "tui" : "chatonly"}`;
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = wantedPanelBody();
    if (panel.dataset.body !== wantedBody) {
      disposeTui();
      disposeSurfaces();
      closeSurfaceMenu?.();
      panel.innerHTML = `${panelHeadHtml(who, shownMode, { removable, hasTerminal, surfaceOptions: surfaceMenuOptionsInFocus() })}
        <div class="rail-body" id="rail-body"></div>
        ${shownMode === "chat" ? `${railViewerHostHtml()}${composerRowHtml()}` : ""}`;
      panel.dataset.head = wantedHead;
      panel.dataset.body = wantedBody;
      wireHead(panel);
      composerControl = null;
      composerModelMenu = null;
      if (shownMode === "tui") mountTui();
      else {
        wireComposer(panel);
        if (autofocusComposerPending) {
          autofocusComposerPending = false;
          panel.querySelector(`#${COMPOSER_IDS.input}`)?.focus();
        }
      }
    } else if (panel.dataset.head !== wantedHead) {
      // The name changed under the panel (an agent whose provider was picked
      // after the fact), or the last agent beside this one went away. Nothing
      // else in the head can move on a poll, and rewriting it every tick would
      // eat a press that landed mid-repaint.
      closeSurfaceMenu?.();
      panel.querySelector(".rail-head").outerHTML = panelHeadHtml(who, shownMode, {
        removable,
        hasTerminal,
        surfaceOptions: surfaceMenuOptionsInFocus(),
      });
      panel.dataset.head = wantedHead;
      wireHead(panel);
    }
    paintSurfaceMenu(panel);
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
        panelModes.set(key, mode);
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
      resetThreadCache();
      threadAgentId = selectedId;
      trySeedThread(); // fire and forget; the refresh under way folds onto it
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
      return saved ? { items: saved.items } : null;
    }
    if (!entity.thread) return null;
    // A payload folds through the cache once; a repaint of the same payload
    // renders the window the cache holds (what absorb would answer anyway).
    // Only a delta that never seated a window re-renders itself as it came.
    if (entity.thread === absorbedThreadPayload) {
      const held = threadCache.readWindow();
      return held ? { ...entity.thread, items: held.items } : { ...entity.thread };
    }
    absorbedThreadPayload = entity.thread;
    const thread = threadCache.absorb(entity.thread);
    persistThreadWindow();
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
  const readOlderItems = async () => {
    const seek = threadCache.olderPageParam();
    if (loadingOlderItems || !seek || !threadCache.hasOlderItems() || !entity.entityId) return;
    loadingOlderItems = true;
    const asked = selectedId;
    try {
      const page = await App.call("thread.page", {
        entity_id: entity.entityId,
        ...(selectedId ? { agent_id: selectedId } : {}),
        ...seek,
      });
      // The reader opened another agent's conversation while this was in
      // flight: it is history from a thread nobody is looking at.
      if (disposed || asked !== selectedId) return;
      // The seek goes back with the page: the cache is the one that knows
      // whether the window it was fetched above is still the window in hand —
      // a poll during this round trip can have reset and reopened it.
      if (threadCache.absorbOlderPage(page, seek)) paintChat({ olderItemsPrepended: true });
    } catch (error) {
      // Scrolling to the top is a deliberate ask, so a refusal is worth
      // saying — unlike a poll, which fails quietly and tries again.
      notifyError("Could not load older messages", error.message);
    } finally {
      loadingOlderItems = false;
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
    body.innerHTML = `<div class="rail-newagent">${providerCardsHtml(creatable().providers, chosen)}</div>`;
    body.dataset.newAgent = chosen;
    body.querySelector(".rail-newagent").onclick = (event) => {
      const card = event.target.closest(".chooser-card");
      if (!card) return;
      // A model belongs to its harness, so moving the highlight drops one
      // chosen under the harness beside it.
      writeNewAgentChoice(
        reconcileAgentChoice({ ...newAgentChoice(), provider: card.dataset.provider }, { providerChanged: true }),
      );
      paintChat();
    };
  };

  const paintChat = ({ olderItemsPrepended = false } = {}) => {
    const body = host.querySelector("#rail-body");
    if (!body) return;
    if (!visibleAgents().length || addingAgent) {
      paintNewAgent(body);
      syncComposer();
      syncSurfaces();
      return;
    }
    const thread = threadFor();
    const agent = agentOf(selectedId);
    paintThreadKeepingPlace(body, () => {
      // No composer in here: the box is pinned below this scroller, so what the
      // poll repaints is the timeline and only the timeline.
      const html = threadHtml(thread || { items: [] }, {
        agentLabel: providerLabel(agent && agent.provider),
      });
      writeThreadKeepingComposer(body, html);
      wireTimeline(body);
    }, { olderItemsPrepended });
    // Assignment rather than a listener: the scroller outlives every repaint,
    // and adding one per paint would ask for the same page once per tick.
    body.onscroll = () => {
      if (body.scrollTop <= OLDER_ITEMS_TRIGGER_PX) readOlderItems();
    };
    syncComposer();
    syncSurfaces();
    reportRead(body);
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
      ${railStatusRowHtml()}
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
  const agentInFocus = () => (addingAgent ? null : agentOf(selectedId));

  const settledAgentInFocus = () => {
    const agent = agentInFocus();
    return agent && !isProvisionalKey(agent.id) ? agent : null;
  };

  const syncComposer = () => {
    if (composerControl) composerControl.setCanInterrupt(agentCanInterrupt(agentInFocus()));
    if (!composerModelMenu) return;
    const choice = composerChoice();
    composerModelMenu.set(catalog, choice.provider, choice, activeModelOf(agentInFocus()));
  };

  /** What the composer's model menu is editing: the open agent's own choice —
   *  its harness names the catalog — or, before there is an agent, what the
   *  first send will create one with. */
  const composerChoice = () => {
    const agent = agentInFocus();
    if (!agent) return newAgentChoice();
    return { provider: agent.provider, model: agent.model || "", effort: agent.effort || "" };
  };

  const activeModelOf = (agent) => (agent && agent.active_model) || "";

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
    await runOptimistic({
      scope: pendingAgentsScope(),
      records: [patchRecord(agent.id, { model: next.model, effort: next.effort })],
      call: () =>
        App.call("agent.choose", {
          entity_id: entity.entityId,
          agent_id: agent.id,
          model: next.model,
          effort: next.effort,
        }),
      failureSummary: "Could not set the model",
    });
    await refresh();
  };

  const wireTimeline = (body) => {
    wireThreadAttachments(body, (path) => App.call("thread.attachment", { entity_id: entity.entityId, path }));
    wireThreadRevisionLinks(body, (revisionId) =>
      App.call("thread.revision", { entity_id: entity.entityId, revision_id: revisionId }),
    );
    wireThreadLinks(body, openLink);
    wireThreadOptions(body, (choice) => choose(choice).catch((error) => {
      notifyError("Choice failed", error.message);
      throw error;
    }));
  };

  /// Wire the pinned box. `panel` rather than the composer row itself, so a file
  /// dropped anywhere on the conversation lands in the tray — the gesture aims
  /// at the agent, not at a 40px strip.
  const wireComposer = (panel) => {
    composerControl = wireThreadComposer(panel, {
      ids: COMPOSER_IDS,
      readDraft: () => draftOf().body,
      writeDraft: (value) => writeDraft({ body: value }),
      readAttachments: () => draftOf().attachments,
      writeAttachments: (next) => writeDraft({ attachments: next }),
      // Attaching lands the bytes before the message names them — which needs a
      // conversation to store them against, so it adopts exactly as sending
      // does: choosing a file for a message is the same intent, one keystroke
      // earlier.
      upload: async (file, contentBase64) => {
        const entityId = await ensureEntity();
        return App.call("thread.attach", { entity_id: entityId, filename: file.name, content_b64: contentBase64 });
      },
      onSubmit: (message, attachments, options) => send(message, attachments, options),
      onError: (error) => notifyError("Message failed", error.message),
    });
    composerModelMenu = mountComposerModelMenu(panel, { ids: COMPOSER_IDS, onChoose: chooseModel });
    mountSurfaces(panel);
    syncComposer();
    syncSurfaces();
  };

  const surfaceViewerCallbacks = () => ({
    onSendMessage: (message) => send(message, []),
    onOpenThreadItem: (sequence) => {
      if (revealThreadSequence(host.querySelector("#rail-body"), sequence)) return;
      notifyError(
        "That call is not in the loaded conversation",
        "Scroll back to load older items, then press the row again.",
      );
    },
  });

  const surfacesInFocus = () => {
    const agent = agentInFocus();
    return (agent && agent.surfaces) || null;
  };

  const surfaceMenuOptionsInFocus = () => surfaceMenuOptions(surfacesInFocus());

  const mountSurfaces = (panel) => {
    const pillHost = panel.querySelector(`#${RAIL_STATUS_PILLS_ID}`);
    const viewerHost = panel.querySelector(`#${RAIL_VIEWER_ID}`);
    if (!pillHost || !viewerHost) return;
    surfacesBlock = mountAgentSurfaces({
      pillHost,
      viewerHost,
      key: conversationKey(),
      onPillsPainted: syncRailStatusRow,
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
    surfacesBlock.set(surfacesInFocus());
  };

  const paintSurfaceMenu = (panel) => {
    const region = panel.querySelector(SURFACE_MENU_SELECTOR);
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
    surfaceOverlay.set(surfacesInFocus());
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
    if (link.kind === "file" && entity.kind === "branch" && entity.branch) {
      go({ name: "branch", projectId: entity.projectId, branch: entity.branch, tab: "files" });
    }
  };

  /// Tell the daemon this agent's conversation has been read, and how much of
  /// it this panel was ever sent.
  ///
  /// Open, in Chat, and scrolled to the end: all three, because a panel showing
  /// the top of a long thread has not read the message at the bottom of it.
  /// The end of the scroller is the end of a WINDOW, though — a long
  /// conversation arrives as a page of its newest items — so the floor of that
  /// window goes with the report. Without it the daemon reads the whole
  /// conversation through, clearing the badge for a message waiting a hundred
  /// items back that this panel never received and nobody ever saw.
  const reportRead = (body) => {
    const agent = agentOf(selectedId);
    if (!agent || !agent.unread_count || !entity.entityId) return;
    if (body.scrollHeight - body.clientHeight - body.scrollTop > 32) return;
    markSeen(entity.entityId, agent.id, threadCache.windowFloorSequence()).then(refreshFeed);
  };

  // ---- sending --------------------------------------------------------------

  /** The entity a message is posted to, adopting the checkout first when Build
   *  owns nothing here yet — an agent needs an owner for `done` to report to. */
  const ensureEntity = async () => {
    if (entity.entityId && !entity.adoptable) return entity.entityId;
    const adopt = adoptingCall();
    if (!adopt) return entity.entityId;
    return adopt.adopt();
  };

  const willCreateAgent = () =>
    !agentInFocus() && entity.kind === "branch" && (!visibleAgents().length || addingAgent);

  const repaintComposerFromDraft = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    panel.dataset.body = "";
    paintPanel();
    const input = panel.querySelector(`#${COMPOSER_IDS.input}`);
    if (input) input.focus();
  };

  const renameAgentIdentity = (fromAgentId, toAgentId) => {
    if (!fromAgentId || !toAgentId || fromAgentId === toAgentId) return;
    const entityKey = entity.entityId || key;
    const draft = drafts.get(`${entityKey}:${fromAgentId}`);
    if (draft) {
      drafts.set(`${entityKey}:${toAgentId}`, draft);
      drafts.delete(`${entityKey}:${fromAgentId}`);
    }
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
    if (selectedId === fromAgentId) chooseAgent(toAgentId);
    adoptPanelBody();
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

  const postMessage = async (handle, { entityId, addressed, message, messageKey, provisionalMessage }) => {
    const posted = await App.call("thread.post", {
      entity_id: entityId,
      ...addressed,
      ...message,
      ...MUTATION_THREAD_PAGE,
    });
    rekeyPostedMessage(handle, messageKey, provisionalMessage, posted);
  };

  const wakeAgent = (entityId, addressed) => App.call("agent.start", { id: entityId, ...addressed });

  const createAgentWithMessage = async (message) => {
    const provisionalAgentId = provisionalKey("agent");
    const provisionalMessageKey = provisionalKey("message");
    const selectedBeforeCreate = selectedId;
    const addingBeforeCreate = addingAgent;
    const choice = newAgentChoice();
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
    const provisionalMessage = provisionalMessageEntry(provisionalMessageKey, message);
    writeDraft({ body: "", attachments: [] });
    addingAgent = false;
    openConversation(provisionalAgentId);
    adoptPanelBody();
    let messageDelivered = false;

    const call = async (handle) => {
      const entityId = await ensureEntity();
      const added = await App.call("agent.add", { entity_id: entityId, ...newAgentParams() });
      const createdAgent = (added && added.agent) || null;
      if (createdAgent) {
        handle.moveScope(pendingThreadScope(provisionalAgentId), pendingThreadScope(createdAgent.id));
        renameAgentIdentity(provisionalAgentId, createdAgent.id);
        handle.rekey(provisionalAgentId, createdAgent.id, { ...provisionalAgent, id: createdAgent.id });
      }
      const addressed = createdAgent ? { agent_id: createdAgent.id } : {};
      await postMessage(handle, {
        entityId,
        addressed,
        message,
        messageKey: provisionalMessageKey,
        provisionalMessage,
      });
      messageDelivered = true;
      await wakeAgent(entityId, addressed);
    };

    const onRevert = () => {
      if (isProvisionalKey(selectedId)) {
        addingAgent = addingBeforeCreate;
        openConversation(selectedBeforeCreate);
      }
      if (messageDelivered) return;
      writeDraft({ body: message.body || "", attachments: message.attachments || [] });
      repaintComposerFromDraft();
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
    creating = settling;
    settling.finally(() => {
      if (creating === settling) creating = null;
    });
  };

  const deliverMessage = async (message) => {
    const messageKey = provisionalKey("message");
    const provisionalMessage = provisionalMessageEntry(messageKey, message);
    const addressedAgentId = selectedId;

    let messageDelivered = false;

    const call = async (handle) => {
      const entityId = await ensureEntity();
      const agent = agentInFocus();
      const addressed = agent ? { agent_id: agent.id } : {};
      await postMessage(handle, { entityId, addressed, message, messageKey, provisionalMessage });
      messageDelivered = true;
      if (entity.kind === "branch" && (!agent || agent.state !== "live")) {
        const started = await wakeAgent(entityId, addressed);
        if (started && started.agent_id) {
          handle.moveScope(pendingThreadScope(addressedAgentId), pendingThreadScope(started.agent_id));
          openConversation(started.agent_id);
        }
      }
    };

    runOptimistic({
      scope: pendingThreadScope(addressedAgentId),
      records: [insertRecord(messageKey, provisionalMessage)],
      call,
      failureSummary: "Message failed",
      onRevert: () => {
        if (messageDelivered) return;
        writeDraft({ body: message.body || "", attachments: message.attachments || [] });
        repaintComposerFromDraft();
      },
    }).then(async () => {
      await refreshFeed();
      await refresh();
    });
    writeDraft({ body: "", attachments: [] });
    paintChat();
  };

  const post = async (message) => {
    if (creating) await creating;
    if (willCreateAgent()) return createAgentWithMessage(message);
    return deliverMessage(message);
  };

  /** A typed message. `interrupt` rides on the post rather than travelling as a
   *  verb of its own: Build never stops a turn without one to put in its place,
   *  and a second round trip is a window in which the agent starts a fresh turn
   *  or finishes. One send path, one flag. */
  const send = (body, attachments, { interrupt = false } = {}) =>
    post({ body, attachments, ...(interrupt ? { interrupt: true } : {}) });

  /** A press on the actions the agent suggested. It goes out as the message it
   *  is — same adoption, same waking, same refresh — and the daemon composes
   *  what the agent hears out of the options it offered. */
  const choose = ({ messageId, optionIds }) =>
    post({ option_reply: { message_id: messageId, option_ids: optionIds } });

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
      if (!newAgentChoices.has(key)) {
        const defaults = loadAgentDefaults();
        writeNewAgentChoice({
          provider: chosenProviderId(creatable(), defaults),
          model: defaults.model || "",
          effort: defaults.effort || "",
        });
      }
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
    if (!(await confirmAction(removeAgentConfirm(agent)))) return;
    if (isPending(pendingAgentsScope(), agent.id)) return;
    const records = [removeRecord(agent.id)];
    const remaining = projectPending(visibleAgents(), records, { keyOf: agentIdOf });
    openConversation(selectAgentId(remaining, null));
    await runOptimistic({
      scope: pendingAgentsScope(),
      records,
      call: () => App.call("agent.remove", { entity_id: entity.entityId, agent_id: agent.id }),
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
    const target = entity.entityId && !entity.adoptable
      ? { id: entity.entityId, ...(agent ? { agent_id: agent.id } : {}) }
      : { project_id: entity.projectId, ...(entity.worktreeId ? { worktree_id: entity.worktreeId } : {}) };
    tui = mountAgentTab(body, target, {
      idleLabel: "No agent session is running here",
      onStart: startAgent,
    });
  };

  /** Put the open agent back on its screen. It names no harness: the agent is
   *  locked to the one it was created on, and its conversation is waiting
   *  there. */
  const startAgent = async () => {
    const agent = agentOf(selectedId);
    let started = null;
    let refusal = null;
    const settled = await runOptimistic({
      scope: pendingAgentsScope(),
      records: agent ? [patchRecord(agent.id, { state: "live" })] : [],
      call: async () => {
        const entityId = await ensureEntity();
        started = await App.call("agent.start", {
          id: entityId,
          ...(agent ? { agent_id: agent.id } : {}),
        });
      },
      notify: false,
      onRevert: (error) => {
        refusal = error;
      },
    });
    if (started && started.agent_id) {
      selectedId = started.agent_id;
      chosenAgent.set(key, selectedId);
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
      releaseFaces();
      host.innerHTML = "";
    },
  };
}
