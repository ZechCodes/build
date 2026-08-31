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
import { NO_AGENT_CHOICE, reconcileAgentChoice } from "./agentChoice.js";
import { confirmAction } from "./confirm.js";
import { composerHtml, mountComposerModelMenu } from "./composer.js";
import { catalogForProvider, modelParams, providerCardsHtml, STARTABLE_PROVIDERS } from "./modelPicker.js";
import { markSeen } from "./inboxView.js";
import { notifyError } from "./notify.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  MUTATION_THREAD_PAGE,
  createThreadCache,
  paintThreadKeepingPlace,
  threadHtml,
  wireThreadAttachments,
  wireThreadOptions,
  wireThreadComposer,
  wireThreadLinks,
  wireThreadRevisionLinks,
  writeThreadKeepingComposer,
} from "./thread.js";
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

/** Pure: the strip's IDENTITY — bubbles top to bottom, the `+` last, carrying
 *  only what cannot change while a bubble lives: which agent it is and which
 *  face it wears. Everything that moves — which one is open, which is working,
 *  what it is waiting on — is written onto the live buttons by syncStripState,
 *  so this string changes only when the AGENTS do.
 *
 *  That is not a micro-optimisation. A bubble's pattern is painted by a renderer
 *  whose clock only runs while its agent works, and an idle bubble holds the
 *  frame it stopped on; replacing the element would take the renderer with it
 *  and start the pattern over every time the agent's news changed. (It is also
 *  what keeps a press that lands mid-repaint from being swallowed by a swapped
 *  button.) */
export function stripHtml(bubbles) {
  return bubbles
    .map((bubble) => {
      const classes = ["rail-bubble", `rail-bubble-${bubble.type}`];
      // A pattern IS the bubble's face, so it takes the label's place: a canvas
      // for core/agentCanvas.js to paint into, named by the ordinal it wears.
      // The `+` and anything else that speaks in a glyph keeps a label.
      const face = bubble.pattern
        ? `<canvas class="rail-glyph" aria-hidden="true"></canvas><span class="rail-count" hidden></span>`
        : `<span class="rail-bubble-label">${esc(bubble.label)}</span>`;
      const pattern = bubble.pattern ? ` data-pattern="${esc(String(bubble.pattern))}"` : "";
      return `<button type="button" class="${classes.join(" ")}" data-bubble="${esc(bubble.type)}"
        data-agent="${esc(bubble.id)}"${pattern}>${face}</button>`;
    })
    .join("");
}

/**
 * Write the moving half onto a strip that is already painted: which bubble is
 * open, which is working, its unread count, and the tooltip that says why.
 * Positional — the strip's own HTML is rebuilt whenever the bubbles themselves
 * change, so index N here is always bubble N there.
 *
 * `faces` are the painters behind the patterns, keyed by `faceKey` — each held
 * with the ink and dimming it was last told, because setting either repaints the
 * held frame, and a poll saying nothing new must not repaint at all. Omitting
 * them syncs the markup alone.
 */
export function syncStripState(strip, bubbles, faces = null) {
  const buttons = strip.querySelectorAll("[data-bubble]");
  bubbles.forEach((bubble, index) => {
    const button = buttons[index];
    if (!button) return;
    button.classList.toggle("active", !!bubble.active);
    button.classList.toggle("working", !!bubble.working);
    button.title = bubble.title;
    button.setAttribute("aria-label", bubble.title);
    const count = button.querySelector(".rail-count");
    if (count) {
      count.textContent = bubble.unread ? String(bubble.unread) : "";
      count.hidden = !bubble.unread;
    }
    const face = faces && faces.get(faceKey(bubble.type, bubble.id));
    if (!face) return;
    face.renderer.setWorking(!!bubble.working);
    // An unread count sits centred on the face, so the face gets out of its
    // way: dimmed, and in the same amber the number is drawn in.
    const unread = bubble.unread > 0;
    const ink = unread ? unreadInk() : restingInk(button);
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

/** Pure: the line pinned above the composer — a pulsing dot and how long the
 *  work item's turn has been running while one is in flight, how far it
 *  stands from upstream, and its diffstat. "" when the status has nothing to
 *  report, which the caller reads as "pin nothing." */
export function railStatusHtml(status) {
  if (!status.working && !status.sync && !status.stat) return "";
  const working = status.working
    ? `<span class="sdot sdot-working"></span><span class="rail-status-working">Working ${esc(status.working)}</span>`
    : "";
  const sync = status.sync ? `<span class="rail-status-sync mono">${esc(status.sync)}</span>` : "";
  const stat = status.stat ? `<span class="rail-status-stat mono">${esc(status.stat)}</span>` : "";
  // The git facts ride one group anchored to the row's end, so the ticking
  // timer widens into open space instead of shoving them along.
  const git = sync || stat ? `<span class="rail-status-git">${sync}${stat}</span>` : "";
  return working + git;
}

/** Pure: the panel's header — who you are talking to, the controls that go with
 *  it (which face of the agent you are looking at, and the way out), and, on an
 *  agent that can be taken back off, the `−` that mirrors the strip's `+`.
 *
 *  `hasTerminal` false drops the TUI button rather than dimming it: an agent
 *  that reports its own work has no basement, so there is nothing behind that
 *  control to offer. The switch is then one button, which still says which face
 *  you are on. */
export function panelHeadHtml(who, mode, { removable = false, hasTerminal = true } = {}) {
  const removeTitle = `Remove ${who} from this branch`;
  const remove = removable
    ? `<button type="button" class="iconbtn rail-remove" title="${esc(removeTitle)}"
        aria-label="${esc(removeTitle)}">−</button>`
    : "";
  const tui = hasTerminal
    ? `<button type="button" class="rail-mode${mode === "tui" ? " on" : ""}" data-mode="tui">TUI</button>`
    : "";
  return `<div class="rail-head">
    <span class="rail-who">${esc(who)}</span>
    <div class="rail-modes" role="group" aria-label="Conversation or terminal">
      <button type="button" class="rail-mode${mode === "chat" ? " on" : ""}" data-mode="chat">Chat</button>
      ${tui}</div>
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
  let threadAgentId = null; // whose conversation the cache holds
  let loadingOlderItems = false; // a page of history is in flight
  // Which agent the payload in hand was READ FOR. Not the same question as
  // threadAgentId: that one is about the cache, this one is about the answer the
  // cache would be filled from. Between opening another agent's bubble and its
  // read landing, the payload still belongs to the agent just left, and its
  // words must not be drawn under the new one's name.
  let threadOwner = null;
  let adopting = null;
  let catalog = null; // models.list, once it lands: the harnesses and their models
  let sending = false; // a first message is adopting/starting — do not repaint over it
  let paintedStrip = null; // the markup the bubble strip currently stands on
  // The painter behind each bubble's face, keyed by `faceKey`, carrying the ink
  // and dimming it was last told. Lives exactly as long as the canvas it paints
  // into — see rebuildFaces.
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

  const agentOf = (id) => entity.agents.find((agent) => agent.id === id) || null;
  /** Open this agent's conversation, and tell everything else on screen: the
   *  bubble strip is the selector for the whole work item, not just the rail. */
  const chooseAgent = (id) => {
    selectedId = id || null;
    if (selectedId) chosenAgent.set(key, selectedId);
    else chosenAgent.delete(key);
    selection.set(selectedId);
  };
  const draftKey = () => `${entity.entityId || key}:${selectedId || "ghost"}`;
  const draftOf = () => drafts.get(draftKey()) || { body: "", attachments: [] };
  const writeDraft = (next) => drafts.set(draftKey(), { ...draftOf(), ...next });

  // ---- the agent that does not exist yet -------------------------------------

  /** What the first send on this work item will create: whatever the human has
   *  said here, over the account's default harness (`models.list`'s
   *  `default_provider`) until a card is pressed. Resolved once, so the cards,
   *  the composer's menu and the `agent.add` params cannot disagree. */
  const newAgentChoice = () => {
    const said = newAgentChoices.get(key) || NO_AGENT_CHOICE;
    return {
      ...said,
      provider: said.provider || (catalog && catalog.default_provider) || STARTABLE_PROVIDERS[0].id,
    };
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

  const paintRailStatus = () => {
    const slot = host.querySelector("#rail-status");
    if (!slot) return;
    const html = railStatusHtml(railWorkStatus(feedRow, Date.now()));
    slot.innerHTML = html;
    slot.hidden = !html;
  };

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
    const scope = { ...threadCache.cursorParam(), ...(selectedId ? { agent_id: selectedId } : {}) };
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
      if (asked && /agent_id/.test((error && error.message) || "")) {
        chooseAgent(null);
        threadCache.reset();
        threadAgentId = null;
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
    if (!answered.agents.length && entity.agents.length && !agentlessOnce) {
      agentlessOnce = true;
      return;
    }
    agentlessOnce = false;
    entity = answered;
    chooseAgent(selectAgentId(entity.agents, selectedId));
    // Whose conversation this payload carries: the agent we asked about, or —
    // when we asked about none, which is every first read — the entity's own,
    // which is the agent the selection just landed on (its first).
    threadOwner = asked === null ? selectedId : asked;
    if (!sending) paint();
  };

  // ---- painting -------------------------------------------------------------

  /// Paint the strip, and put the panel in or take it out.
  ///
  /// The strip is rewritten whenever what it SAYS changes — and only then.
  /// Nearly every tick resolves the same agents, and rewriting the buttons
  /// under a press swaps the element the pointer went down on for an identical
  /// one, which swallows the press. The PANEL element is never rewritten by a
  /// poll at all: it is where a live PTY hangs, and replacing it would tear a
  /// terminal down and re-attach it every second and a half. So the panel is
  /// created when the human opens it, removed when they shut it, and otherwise
  /// left alone.
  /// One painter per canvas on the strip that was just written, and none left
  /// over from the one it replaced.
  ///
  /// The canvases are new elements, so the painters have to be new too — a
  /// renderer holds the context of the canvas it was made for. That is why the
  /// strip's markup is rewritten only when the AGENTS change: every rewrite is a
  /// pattern starting over, and a poll must never cause one.
  const rebuildFaces = (strip) => {
    faces.forEach((face) => face.renderer.destroy());
    faces.clear();
    strip.querySelectorAll("[data-pattern]").forEach((button) => {
      const canvas = button.querySelector("canvas.rail-glyph");
      if (!canvas) return;
      const renderer = createPatternRenderer({
        canvas,
        patternIndex: Number(button.dataset.pattern),
        seed: patternSeed(button.dataset.agent || ""),
      });
      faces.set(faceKey(button.dataset.bubble, button.dataset.agent), {
        renderer,
        ink: null,
        dimmed: false,
      });
    });
  };

  const paint = () => {
    if (disposed) return;
    if (!host.querySelector(".rail-strip")) {
      host.innerHTML = `<div class="rail-strip"></div>`;
      paintedStrip = null;
    }
    const strip = host.querySelector(".rail-strip");
    const bubbles = railBubbles({ agents: entity.agents, selectedId, kind: entity.kind });
    const wantedStrip = stripHtml(bubbles);
    if (paintedStrip !== wantedStrip) {
      strip.innerHTML = wantedStrip;
      paintedStrip = wantedStrip;
      strip.querySelectorAll("[data-bubble]").forEach((bubble) => {
        bubble.onclick = () => pressBubble(bubble.dataset.bubble, bubble.dataset.agent);
      });
      rebuildFaces(strip);
    }
    // The news goes onto the buttons that are there — see stripHtml.
    syncStripState(strip, bubbles, faces);
    let panel = host.querySelector("#rail-panel");
    if (expanded && !panel) {
      panel = document.createElement("div");
      panel.className = "rail-panel";
      panel.id = "rail-panel";
      host.insertBefore(panel, strip);
    } else if (!expanded && panel) {
      disposeTui();
      panel.remove();
    }
    if (expanded) paintPanel();
  };

  const paintPanel = () => {
    const panel = host.querySelector("#rail-panel");
    if (!panel) return;
    const agent = agentOf(selectedId);
    const who = agent ? agentTitle(agent) : entity.kind === "issue" ? "Issue agent" : "New agent";
    const removable = canRemoveAgent({ agents: entity.agents, agentId: selectedId, kind: entity.kind });
    const hasTerminal = agentHasTerminal(agent);
    // Which face this agent can actually wear. `mode` is remembered per work
    // item, so opening a terminal-less agent's bubble — or one whose digest
    // stopped offering a terminal under an open panel — arrives holding "tui"
    // for a screen that does not exist. The remembered choice is kept rather
    // than rewritten, so the agent beside it that does have a terminal is still
    // where the human left it.
    const shownMode = hasTerminal ? mode : "chat";
    // The head is rewritten only when what it SAYS changed: the name, whether
    // this agent can be taken back off, and whether it has a basement.
    const wantedHead = `${who}:${removable ? "removable" : "kept"}:${hasTerminal ? "tui" : "chatonly"}`;
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = `${shownMode}:${selectedId || "ghost"}`;
    if (panel.dataset.body !== wantedBody) {
      disposeTui();
      panel.innerHTML = `${panelHeadHtml(who, shownMode, { removable, hasTerminal })}
        <div class="rail-body" id="rail-body"></div>
        ${shownMode === "chat" ? composerRowHtml() : ""}`;
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
      panel.querySelector(".rail-head").outerHTML = panelHeadHtml(who, shownMode, { removable, hasTerminal });
      panel.dataset.head = wantedHead;
      wireHead(panel);
    }
    if (shownMode === "chat") {
      paintChat();
      paintRailStatus();
    }
  };

  const wireHead = (panel) => {
    panel.querySelectorAll("[data-mode]").forEach((control) => {
      control.onclick = () => {
        // The terminal is asked for through a button the head only draws for an
        // agent that has one, so a press cannot name a face this agent cannot
        // wear — paintPanel decides that, and this only records the choice.
        if (mode === control.dataset.mode) return;
        mode = control.dataset.mode;
        panelModes.set(key, mode);
        paintPanel();
      };
    });
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

  const threadFor = () => {
    // The cache holds one conversation; switching bubbles switches which.
    if (threadAgentId !== selectedId) {
      threadCache.reset();
      threadAgentId = selectedId;
    }
    // The payload in hand belongs to the agent it was read for. Just after a
    // switch that is the agent just left, and absorbing it would refill the
    // cache the switch cleared with the wrong conversation — which is exactly
    // what made switching look like it did nothing. Nothing until the read for
    // THIS agent lands; pressBubble asks for it immediately.
    if (threadOwner !== selectedId) return null;
    return entity.thread ? threadCache.absorb(entity.thread) : null;
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
    body.innerHTML = `<div class="rail-newagent">${providerCardsHtml(STARTABLE_PROVIDERS, chosen)}</div>`;
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
    if (!entity.agents.length) {
      paintNewAgent(body);
      syncComposer();
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
    reportRead(body);
  };

  const composerPlaceholder = () =>
    entity.agents.length ? "Send a message to this agent…" : "Send a message to start an agent here…";

  /// The box you write in, pinned below the conversation instead of sitting at
  /// the end of it. It is a SIBLING of the scroller, so reading back through a
  /// long thread never takes the box off screen, and the growth of a box being
  /// typed into comes out of the thread above rather than pushing its own
  /// bottom edge past the panel.
  const composerRowHtml = () =>
    `<div class="rail-composer" id="rail-composer">
      <div class="rail-status" id="rail-status" hidden></div>
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
  const syncComposer = () => {
    if (composerControl) composerControl.setCanInterrupt(agentCanInterrupt(agentOf(selectedId)));
    if (!composerModelMenu) return;
    const choice = composerChoice();
    composerModelMenu.set(catalog, choice.provider, choice);
  };

  /** What the composer's model menu is editing: the open agent's own choice —
   *  its harness names the catalog — or, before there is an agent, what the
   *  first send will create one with. */
  const composerChoice = () => {
    const agent = agentOf(selectedId);
    if (!agent) return newAgentChoice();
    return { provider: agent.provider, model: agent.model || "", effort: agent.effort || "" };
  };

  /** A model or effort picked from that menu.
   *
   *  With an agent it is the entity's persisted choice, which the NEXT start
   *  spends — the session running right now is never touched, which is what
   *  makes the menu safe to press mid-turn. With none there is nothing on the
   *  bridge to write to yet, so it waits for the send that creates one. */
  const chooseModel = async (next) => {
    if (!agentOf(selectedId)) {
      writeNewAgentChoice(next);
      return;
    }
    try {
      await App.call("agent.choose", { entity_id: entity.entityId, model: next.model, effort: next.effort });
    } catch (error) {
      notifyError("Could not set the model", error.message);
    }
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
    syncComposer();
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

  /** The agent a message on an agentless branch is for: the one the send
   *  creates, on the harness the new-agent view has highlighted. An issue
   *  dispatches its own planning agent on its first message, so it needs
   *  none of this. */
  const agentForMessage = async (entityId) => {
    const open = agentOf(selectedId);
    if (open || entity.kind !== "branch" || entity.agents.length) return open;
    return addAgent({ entity_id: entityId, ...newAgentParams() });
  };

  /**
   * Send, and make sure something is listening.
   *
   * A message is durable the moment it is posted; whether an agent hears it is
   * a second question. On a branch with no live session — including one whose
   * agent this send has just created — the start is what delivers it, and it
   * answers with the agent that now owns this conversation. An issue needs none
   * of that: the daemon dispatches its planning agent on the first message.
   */
  const post = async (message) => {
    sending = true;
    try {
      const entityId = await ensureEntity();
      const agent = await agentForMessage(entityId);
      await App.call("thread.post", {
        entity_id: entityId,
        ...(agent ? { agent_id: agent.id } : {}),
        ...message,
        ...MUTATION_THREAD_PAGE,
      });
      if (entity.kind === "branch" && (!agent || agent.state !== "live")) {
        const started = await App.call("agent.start", {
          id: entityId,
          ...(agent ? { agent_id: agent.id } : {}),
        });
        if (started && started.agent_id) {
          chooseAgent(started.agent_id);
          threadCache.reset();
          threadAgentId = selectedId;
        }
      }
    } finally {
      sending = false;
    }
    await refreshFeed();
    await refresh();
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
    if (type === "agent" && agentId && agentId !== selectedId) {
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
    chooseAgent(agentId);
    threadCache.reset();
    threadAgentId = agentId;
    expanded = true;
    writeExpanded(true);
  };

  /** Put an agent on this branch and open it. The `+` bubble and the
   *  new-agent view's first send are the same act — a harness named, an agent
   *  created, its conversation opened — so they compose it here. */
  const addAgent = async (params) => {
    const added = await App.call("agent.add", params);
    const agent = (added && added.agent) || null;
    if (agent) openAgent(agent.id);
    return agent;
  };

  /** Another agent on this branch, beside the ones already here. It starts on
   *  the harness this browser prefers (an empty preference sends nothing and
   *  the daemon's own default stands) and nothing runs until it is spoken to. */
  const pressAddBubble = async () => {
    if (!entity.entityId) return;
    const defaults = loadAgentDefaults();
    const params = { entity_id: entity.entityId };
    for (const field of ["provider", "model", "effort"]) {
      if (defaults[field]) params[field] = defaults[field];
    }
    try {
      await addAgent(params);
      await refresh();
    } catch (error) {
      notifyError("Could not add an agent", error.message);
    }
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
    const agent = agentOf(selectedId);
    if (!agent || !entity.entityId) return;
    if (!(await confirmAction(removeAgentConfirm(agent)))) return;
    try {
      await App.call("agent.remove", { entity_id: entity.entityId, agent_id: agent.id });
    } catch (error) {
      notifyError("Could not remove the agent", error.message);
      return;
    }
    // Let the choice go rather than naming the agent that just stopped
    // existing: the next read opens the rail on whichever agent is left, and
    // tells the surfaces beside it the same.
    chooseAgent(null);
    threadCache.reset();
    threadAgentId = null;
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
      onStart: (provider) => startAgent(provider),
    });
  };

  const startAgent = async (provider) => {
    const entityId = await ensureEntity();
    const agent = agentOf(selectedId);
    const started = await App.call("agent.start", {
      id: entityId,
      ...(agent ? { agent_id: agent.id } : {}),
      ...(provider ? { provider } : {}),
    });
    if (started && started.agent_id) {
      selectedId = started.agent_id;
      chosenAgent.set(key, selectedId);
    }
    await refresh();
    return started;
  };

  const disposeTui = () => {
    if (!tui) return;
    tui.dispose();
    tui = null;
  };

  // ---- lifecycle ------------------------------------------------------------

  paint();
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
      unsubscribeFeed();
      disposeTui();
      faces.forEach((face) => face.renderer.destroy());
      faces.clear();
      host.innerHTML = "";
    },
  };
}
