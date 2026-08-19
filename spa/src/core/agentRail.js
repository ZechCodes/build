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

import { App, go } from "../app.js";
import { watchChanges } from "./changeEvents.js";
import { createAdoptingCall, createPrimaryAdoptingCall } from "./adoption.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import {
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
import { confirmAction } from "./confirm.js";
import { composerHtml } from "./composer.js";
import { markSeen } from "./inboxView.js";
import { notifyError } from "./notify.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { toolbarIdentity } from "./toolbarModel.js";
import { esc } from "./text.js";
import {
  createThreadCache,
  paintThreadKeepingPlace,
  threadHtml,
  wireThreadAttachments,
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

const EXPANDED_KEY = "build.rail.expanded";
const COMPOSER_IDS = { input: "railinput", send: "railsend", hint: "railhint" };

// What survives a remount. The rail is rebuilt whenever the view under it is
// (a tab switch re-renders the surface), so the human's choices — which agent
// is open, whether the panel is out, chat or TUI, and anything typed but not
// sent — are kept here rather than in the DOM that is about to be replaced.
const drafts = new Map(); // `${entityId}:${agentId}` → { body, attachments }
const chosenAgent = new Map(); // entity key → agent id the human last opened
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
 *  That is not a micro-optimisation. A bubble's pattern animation is paused
 *  while its agent is idle, and it holds the frame it stopped on; replacing the
 *  element would restart it from the beginning every time the agent's news
 *  changed. (It is also what keeps a press that lands mid-repaint from being
 *  swallowed by a swapped button.) */
export function stripHtml(bubbles) {
  return bubbles
    .map((bubble) => {
      const classes = ["rail-bubble", `rail-bubble-${bubble.type}`];
      if (bubble.pattern) classes.push(`rail-pattern-${bubble.pattern}`);
      // A pattern IS the bubble's face, so it takes the label's place. The `+`
      // and anything else that speaks in a glyph keeps one.
      const face = bubble.pattern
        ? `<span class="rail-glyph" aria-hidden="true"></span>`
        : `<span class="rail-bubble-label">${esc(bubble.label)}</span>`;
      return `<button type="button" class="${classes.join(" ")}" data-bubble="${esc(bubble.type)}"
        data-agent="${esc(bubble.id)}">
        ${face}<span class="rail-badge" hidden></span></button>`;
    })
    .join("");
}

/** Write the moving half onto a strip that is already painted: which bubble is
 *  open, which is working, its unread count, and the tooltip that says why.
 *  Positional — the strip's own HTML is rebuilt whenever the bubbles themselves
 *  change, so index N here is always bubble N there. */
export function syncStripState(strip, bubbles) {
  const buttons = strip.querySelectorAll("[data-bubble]");
  bubbles.forEach((bubble, index) => {
    const button = buttons[index];
    if (!button) return;
    button.classList.toggle("active", !!bubble.active);
    button.classList.toggle("working", !!bubble.working);
    button.title = bubble.title;
    button.setAttribute("aria-label", bubble.title);
    const badge = button.querySelector(".rail-badge");
    if (!badge) return;
    badge.textContent = bubble.unread ? String(bubble.unread) : "";
    badge.hidden = !bubble.unread;
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
  return working + sync + stat;
}

/** Pure: the panel's header — who you are talking to, the two controls that are
 *  always there (which face of the agent you are looking at, and the way out),
 *  and, on an agent that can be taken back off, the `−` that mirrors the strip's
 *  `+`. */
export function panelHeadHtml(who, mode, { removable = false } = {}) {
  const removeTitle = `Remove ${who} from this branch`;
  const remove = removable
    ? `<button type="button" class="iconbtn rail-remove" title="${esc(removeTitle)}"
        aria-label="${esc(removeTitle)}">−</button>`
    : "";
  return `<div class="rail-head">
    <span class="rail-who">${esc(who)}</span>
    <div class="rail-modes" role="group" aria-label="Conversation or terminal">
      <button type="button" class="rail-mode${mode === "chat" ? " on" : ""}" data-mode="chat">Chat</button>
      <button type="button" class="rail-mode${mode === "tui" ? " on" : ""}" data-mode="tui">TUI</button>
    </div>
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
  // Which agent the payload in hand was READ FOR. Not the same question as
  // threadAgentId: that one is about the cache, this one is about the answer the
  // cache would be filled from. Between opening another agent's bubble and its
  // read landing, the payload still belongs to the agent just left, and its
  // words must not be drawn under the new one's name.
  let threadOwner = null;
  let adopting = null;
  let sending = false; // a first message is adopting/starting — do not repaint over it
  let paintedStrip = null; // the markup the bubble strip currently stands on
  let agentlessOnce = false; // an answer that lost the agents, waiting to be repeated
  let feedRow = null; // this work item's row off the shared feed, for the pinned status line
  let statusTicker = null;
  // One-shot: the composer steals focus the first time it paints, then never
  // again — a poll rebuilding the panel later (a new agent, a mode switch)
  // must not keep yanking focus back while the human is doing something else.
  let autofocusComposerPending = context.autofocusComposer === true;

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
    }
    // The news goes onto the buttons that are there — see stripHtml.
    syncStripState(strip, bubbles);
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
    // The head is rewritten only when what it SAYS changed: the name, and
    // whether this agent can be taken back off.
    const wantedHead = `${who}:${removable ? "removable" : "kept"}`;
    // The body is rebuilt only when what it is showing changed — which face of
    // the agent, and which agent. Same reason as the panel itself.
    const wantedBody = `${mode}:${selectedId || "ghost"}`;
    if (panel.dataset.body !== wantedBody) {
      disposeTui();
      panel.innerHTML = `${panelHeadHtml(who, mode, { removable })}
        <div class="rail-body" id="rail-body"></div>
        ${mode === "chat" ? composerRowHtml() : ""}`;
      panel.dataset.head = wantedHead;
      panel.dataset.body = wantedBody;
      wireHead(panel);
      if (mode === "tui") mountTui();
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
      panel.querySelector(".rail-head").outerHTML = panelHeadHtml(who, mode, { removable });
      panel.dataset.head = wantedHead;
      wireHead(panel);
    }
    if (mode === "chat") {
      paintChat();
      paintRailStatus();
    }
  };

  const wireHead = (panel) => {
    panel.querySelectorAll("[data-mode]").forEach((control) => {
      control.onclick = () => {
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

  const paintChat = () => {
    const body = host.querySelector("#rail-body");
    if (!body) return;
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
    });
    syncComposerPlaceholder();
    reportRead(body);
  };

  const composerPlaceholder = () => {
    if (entity.adoptable) return "Send a message to start an agent here…";
    if (!entity.agents.length) return "Send a message to start the agent…";
    return "Send a message to this agent…";
  };

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
      })}</div>`;

  /// The placeholder is the only thing on the composer a poll can change — a
  /// checkout Build owned nothing in a second ago now has an agent to talk to.
  /// The element itself is never rebuilt, so the words are moved onto it.
  const syncComposerPlaceholder = () => {
    const input = host.querySelector(`#${COMPOSER_IDS.input}`);
    if (input) input.placeholder = composerPlaceholder();
  };

  const wireTimeline = (body) => {
    wireThreadAttachments(body, (path) => App.call("thread.attachment", { entity_id: entity.entityId, path }));
    wireThreadRevisionLinks(body, (revisionId) =>
      App.call("thread.revision", { entity_id: entity.entityId, revision_id: revisionId }),
    );
    wireThreadLinks(body, openLink);
  };

  /// Wire the pinned box. `panel` rather than the composer row itself, so a file
  /// dropped anywhere on the conversation lands in the tray — the gesture aims
  /// at the agent, not at a 40px strip.
  const wireComposer = (panel) => {
    wireThreadComposer(panel, {
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
      onSubmit: (message, attachments) => send(message, attachments),
      onError: (error) => notifyError("Message failed", error.message),
    });
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

  /// Tell the daemon this agent's conversation has been read.
  ///
  /// Open, in Chat, and scrolled to the end: all three, because a panel showing
  /// the top of a long thread has not read the message at the bottom of it.
  const reportRead = (body) => {
    const agent = agentOf(selectedId);
    if (!agent || !agent.unread_count || !entity.entityId) return;
    if (body.scrollHeight - body.clientHeight - body.scrollTop > 32) return;
    markSeen(entity.entityId, agent.id).then(refreshFeed);
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

  /**
   * Send, and make sure something is listening.
   *
   * A message is durable the moment it is posted; whether an agent hears it is
   * a second question. On a branch with no live session — including one with no
   * agent at all, where the post has just adopted the checkout — the start is
   * what delivers it, and it answers with the agent that now owns this
   * conversation. An issue needs none of that: the daemon dispatches its
   * planning agent on the first message.
   */
  const send = async (body, attachments) => {
    sending = true;
    try {
      const entityId = await ensureEntity();
      const agent = agentOf(selectedId);
      await App.call("thread.post", {
        entity_id: entityId,
        ...(agent ? { agent_id: agent.id } : {}),
        body,
        attachments,
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

  // ---- the strip's presses --------------------------------------------------

  const pressBubble = (type, agentId) => {
    if (type === "add") {
      addAgent();
      return;
    }
    if (type === "agent" && agentId && agentId !== selectedId) {
      chooseAgent(agentId);
      threadCache.reset();
      threadAgentId = agentId;
      expanded = true;
      writeExpanded(true);
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

  /** Another agent on this branch, with its own conversation. It starts on the
   *  account's chosen harness (an empty preference sends nothing and the
   *  daemon's own default stands) and nothing runs until it is spoken to. */
  const addAgent = async () => {
    if (!entity.entityId) return;
    const defaults = loadAgentDefaults();
    const params = { entity_id: entity.entityId };
    for (const field of ["provider", "model", "effort"]) {
      if (defaults[field]) params[field] = defaults[field];
    }
    try {
      const added = await App.call("agent.add", params);
      if (added && added.agent) {
        selectedId = added.agent.id;
        chosenAgent.set(key, selectedId);
        threadCache.reset();
        threadAgentId = selectedId;
        expanded = true;
        writeExpanded(true);
      }
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
      host.innerHTML = "";
    },
  };
}
