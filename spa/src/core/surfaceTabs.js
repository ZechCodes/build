// The agent's PTY, as a mountable body.
//
// One agent screen per worktree, addressed the way the calling surface knows
// that worktree, with the idle offer laid over it when nothing is running. The
// agent rail's TUI mode is what mounts it; the human's own shells are the
// console's (core/console.js), and the two share only the connectivity overlay
// below. Panes ride the ONE shared terminal socket (manager.js), demuxed by
// term_id.

import { terminalManager, subscribeTerminalStatus } from "../terminal/manager.js";
import { mountTerminalPane } from "../terminal/pane.js";
import { DEFAULT_START_PROVIDER, STARTABLE_PROVIDERS, providerCardsHtml } from "./modelPicker.js";

/**
 * Overlay a `.termpane` host with a connectivity chip + dim-while-offline, driven
 * by the shared terminal socket's status (subscribeTerminalStatus). A frozen
 * pane now says why. Same visual family as the agent-idle chip (a small pill
 * floated over the screen). Returns { dispose() } — unsubscribe + remove the chip.
 * Call AFTER the pane has mounted, so ghostty's own innerHTML reset (pane.js)
 * doesn't wipe the chip.
 */
export function attachConnectionOverlay(host) {
  const chip = document.createElement("div");
  chip.className = "term-conn";
  chip.hidden = true;
  chip.textContent = "reconnecting to your machine…";
  host.appendChild(chip);
  const unsubscribe = subscribeTerminalStatus((status) => {
    const offline = status !== "connected";
    chip.hidden = !offline;
    host.classList.toggle("term-offline", offline);
  });
  return {
    dispose() {
      unsubscribe();
      chip.remove();
      host.classList.remove("term-offline");
    },
  };
}

/** Mount a worktree's agent pane.
 *
 *  `target` is how the calling surface addresses that worktree: `{ id }` for a
 *  run or plan, or the scope of the directory itself (`{ run_id }`,
 *  `{ project_id, worktree_id }`, `{ project_id }`). The WIRE id comes back from
 *  the attach — it is `agent:<worktree_id>`, a hash of the canonical root that
 *  no client can compute — and every keystroke, resize and detach after that
 *  uses it. `onLive(bool, attachResult)` reports session liveness (the second
 *  argument is the attach payload, so a caller can tell a dead-with-a-screen
 *  agent from one that never ran);
 *  `onExit(reason)` reacts to closures. Input is allowed — a live PTY on the
 *  user's machine. */
export function mountAgentPane(host, target, { onLive, onExit }) {
  const manager = terminalManager();
  let termId = null; // learned from the attach; null until then, and after a failed one
  return mountTerminalPane(host, {
    attach: (opts) =>
      // onTermId, not just the attach result: a screen mounted on a worktree
      // with no agent is keyed by the WORKTREE until one is born, and the
      // socket follows the newborn's id (session.js). Keystrokes have to
      // follow it too.
      manager.attachAgent(target, { ...opts, onLive, onTermId: (id) => { termId = id; } }).then((r) => {
        termId = r.term_id;
        return r;
      }),
    // Before the attach lands there is no screen to type into. Rejecting (rather
    // than dropping) routes through onInputError, so the idle chip shows instead
    // of the keystroke silently vanishing.
    input: (data) => (termId ? manager.input(termId, data) : Promise.reject(new Error("no active agent session"))),
    resize: (cols, rows) => (termId ? manager.resize(termId, cols, rows) : Promise.resolve()),
    onExit,
    // A keystroke rejected by the bridge means the session is gone — surface it
    // as the idle state (the same reason a live session's close reports), so the
    // "no active agent session" chip shows instead of the input silently
    // vanishing. Non-destructive: a new session's first frame clears it (B1).
    onInputError: () => onExit && onExit("agent_session_ended"),
  }).then((pane) => {
    // Overlay the connectivity chip once the pane exists (its mount resets the
    // host's innerHTML). Fold its teardown into the pane's own dispose.
    const conn = attachConnectionOverlay(host);
    return {
      ...pane,
      dispose() {
        conn.dispose();
        pane.dispose();
        // Deregister the screen the attach named — the server PTY keeps running.
        if (termId) manager.detach(termId);
      },
    };
  });
}

/** Mount the Agent tab's BODY for a worktree: its one agent as a full PTY, with
 *  a quiet chip whenever no session is live.
 *
 *  The same body on every worktree surface (task, external worktree, primary
 *  checkout, plan). Mounting it starts NOTHING: a worktree no agent has run in
 *  attaches to an empty screen and shows `idleLabel`. An agent begins when a
 *  human→agent verb delivers a turn, never because a tab was opened.
 *
 *  `onStart(provider)` receives the harness the human picked — always named,
 *  from whichever of the three controls was pressed. `selectedProvider` marks
 *  the card an earlier answer already chose. Omitting `onStart` leaves the tab
 *  a viewer.
 *
 *  Returns { dispose() } — tears down the client view only. */
export function mountAgentTab(
  host,
  target,
  {
    idleLabel = "No agent is currently running",
    onStart,
    selectedProvider = null,
  } = {},
) {
  host.innerHTML = `<div class="agentwrap">
    <div class="termpane" id="agentpane"></div>
    <div class="agent-overlay" id="agentOverlay" hidden>
      <p class="agent-overlay-msg" id="agentOverlayMsg"></p>
      ${onStart ? `<div id="agentStartChoices" hidden></div>` : ""}
    </div>
  </div>`;
  const shade = host.querySelector("#agentOverlay");
  const message = host.querySelector("#agentOverlayMsg");
  const choices = host.querySelector("#agentStartChoices");
  const labelOf = (card) => card.querySelector(".chooser-card-label");
  const providerLabel = (id) => STARTABLE_PROVIDERS.find((provider) => provider.id === id).label;
  const allCards = () => (choices ? [...choices.querySelectorAll(".chooser-card")] : []);

  // The harness this worktree's tab last ran, as the attach reported it — the
  // retained screen of a dead agent is the only record of which one painted it,
  // and the entity's own model choice can have moved since.
  let ranProvider = null;

  // The offer, in both silences: the obvious harness as a wide button, with
  // both providers under it. A restart IS a start — what exiting changes is
  // which one is obvious, and that is a label, not a different control.
  const renderChoices = () => {
    const leadProvider = ranProvider || DEFAULT_START_PROVIDER;
    choices.hidden = false;
    choices.innerHTML = providerCardsHtml(STARTABLE_PROVIDERS, selectedProvider, {
      leadId: "agentStartLead",
      lead: {
        id: leadProvider,
        label: providerLabel(leadProvider),
        description: ranProvider ? "ran here last" : "the default",
      },
    });
  };

  // Which silence this is. `exited` is the one with a screen behind it worth
  // reading, so its overlay is laid OVER that screen instead of standing in a
  // blank pane.
  let exited = false;
  const show = (state, reason) => {
    if (state === "live") {
      shade.hidden = true;
      return;
    }
    exited = state === "exited";
    shade.hidden = false;
    shade.classList.toggle("over-screen", exited);
    // An exited state says nothing of its own: the retained screen behind the
    // overlay is the explanation, and the picker is the whole offer. The line
    // still speaks for a start failure (reason) and for the inert idle pane.
    message.textContent = reason || (exited ? "" : idleLabel);
    message.hidden = !message.textContent;
    // Re-rendering IS the reset: every label and disabled flag comes back with
    // the fresh markup, so a failed start needs no cleanup of its own.
    if (onStart) renderChoices();
  };

  // One start path for all three controls. `busy` is what the pressed one says
  // while the harness comes up; every other control goes inert, since a second
  // press in flight would race two harnesses over one worktree.
  const startAgent = async (provider, busy) => {
    for (const card of allCards()) card.disabled = true;
    busy();
    try {
      await onStart(provider);
      // The agent is up. Its first frame would clear this anyway (the pane is
      // already attached to the screen it is born onto), but not waiting for a
      // round trip is what makes the press feel like it did something.
      shade.hidden = true;
    } catch (e) {
      // Standing offer, plus the reason — a start that failed silently would
      // leave the human pressing a control that never explains itself.
      show(exited ? "exited" : "idle", (e && e.message) || "could not start the agent");
    }
  };

  // Delegated, because the offer is re-rendered on every state change: the
  // pressed card names its own harness, so no control ever starts an unnamed
  // one.
  if (choices) {
    choices.onclick = (event) => {
      const card = event.target.closest(".chooser-card");
      if (!card || card.disabled) return;
      startAgent(card.dataset.provider, () => {
        labelOf(card).textContent = "Starting…";
      });
    };
  }

  let pane = null;
  let disposed = false;
  mountAgentPane(host.querySelector("#agentpane"), target, {
    // A retained screen with no live session is a harness that ran and stopped;
    // an empty one never ran at all. Either way the attach names the harness
    // the tab runs, which is what the offer leads with once it is gone.
    onLive: (live, attached) => {
      if (attached && attached.provider) ranProvider = attached.provider;
      show(live ? "live" : hasScreen(attached) ? "exited" : "idle");
    },
    onExit: (reason) => {
      if (reason === "agent_session_ended") show("exited");
    },
  }).then(
    (p) => (disposed ? p.dispose() : (pane = p)),
    // An unresolvable address (a plan whose worktree is gone, a deleted run) is
    // the empty state too — the offer stands, the surface is intact.
    () => show("idle"),
  );
  return {
    dispose() {
      disposed = true;
      if (pane) pane.dispose();
    },
  };
}

/** Whether an attach came back with a screen the human can still read. */
function hasScreen(attached) {
  return !!(attached && attached.snapshot && attached.snapshot.length);
}
