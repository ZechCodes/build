// The agent's PTY, as a mountable body.
//
// One agent screen per worktree, addressed the way the calling surface knows
// that worktree, with the idle offer laid over it when nothing is running. The
// agent rail's TUI mode is what mounts it; the human's own shells are the
// console's (core/console.js), and the two share only the connectivity overlay
// below. Panes ride the ONE shared terminal socket (manager.js), demuxed by
// term_id.

import { terminalManager, subscribeTerminalStatus } from "../terminal/manager.js";
import { isTerminalSocketLost } from "../terminal/session.js";
import { mountTerminalPane } from "../terminal/pane.js";

/** What every surface says while the machine is out of reach. One sentence, in
 *  the chip over a live pane and in the place of a pane that could not attach. */
export const RECONNECTING_MESSAGE = "reconnecting to your machine…";

/**
 * Run `retry` the next time the shared terminal socket reports a connection —
 * immediately, if it already has one. Returns { dispose() }; the watch is
 * one-shot and disposing it before the socket returns cancels the retry.
 *
 * The socket owns the reconnect and its backoff, so a surface whose attach was
 * cut off waits for THAT rather than retrying on a schedule of its own: retries
 * on a timer are what piled up behind a socket that was never coming back.
 */
export function whenTerminalReconnects(retry) {
  let waiting = true; // still expecting a connection
  let disposed = false;
  let unsubscribe = null;
  const release = () => {
    if (unsubscribe) unsubscribe();
    unsubscribe = null;
  };
  unsubscribe = subscribeTerminalStatus((status) => {
    if (!waiting || status !== "connected") return;
    waiting = false;
    // Off the subscribe call: the hub reports the current status DURING
    // subscribe, and running the retry inline would fire before there is an
    // unsubscribe to release — leaving the watch behind.
    setTimeout(() => {
      release();
      if (!disposed) retry();
    }, 0);
  });
  return {
    dispose() {
      disposed = true;
      waiting = false;
      release();
    },
  };
}

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
  chip.textContent = RECONNECTING_MESSAGE;
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
 *  The same body on every worktree surface (task, external worktree, plain
 *  checkout, plan). Mounting it starts NOTHING: a worktree no agent has run in
 *  attaches to an empty screen and shows `idleLabel`. An agent begins when a
 *  human→agent verb delivers a turn, never because a tab was opened.
 *
 *  `onStart()` puts the agent back on its screen, naming no harness: the pane
 *  belongs to an agent that already exists, and an agent is locked to the
 *  harness it was created on for the life of its conversation. Omitting
 *  `onStart` leaves the tab a viewer.
 *
 *  Returns { dispose() } — tears down the client view only. */
export function mountAgentTab(host, target, { idleLabel = "No agent is currently running", onStart } = {}) {
  host.innerHTML = `<div class="agentwrap">
    <div class="termpane" id="agentpane"></div>
    <div class="agent-overlay" id="agentOverlay" hidden>
      <p class="agent-overlay-msg" id="agentOverlayMsg"></p>
      ${onStart ? `<div id="agentStartOffer" hidden></div>` : ""}
    </div>
  </div>`;
  const shade = host.querySelector("#agentOverlay");
  const message = host.querySelector("#agentOverlayMsg");
  const startOffer = host.querySelector("#agentStartOffer");
  const resumeButton = () => startOffer && startOffer.querySelector("#agentResume");

  // The offer, in both silences: one button. Which harness it opens is not a
  // question — the agent is locked to the one it was created on, and its
  // conversation is waiting there.
  const renderStartOffer = () => {
    startOffer.hidden = false;
    startOffer.innerHTML = `<button type="button" class="btn primary agent-resume" id="agentResume">Resume</button>`;
  };

  // Which silence this is. `exited` is the one with a screen behind it worth
  // reading, so its overlay is laid OVER that screen instead of standing in a
  // blank pane.
  let exited = false;
  // A start the human pressed is in flight. The socket can report a state
  // change while it runs (a session flapping out from under the press), and
  // re-rendering then would replace the card saying "Starting…" with an idle
  // one — the press would look like it never happened.
  let starting = false;
  const show = (state, reason) => {
    if (state === "live") {
      shade.hidden = true;
      return;
    }
    exited = state === "exited";
    shade.hidden = false;
    shade.classList.toggle("over-screen", exited);
    // An exited state says nothing of its own: the retained screen behind the
    // overlay is the explanation, and the resume button is the whole offer.
    // The line still speaks for a start failure (reason) and for the inert
    // idle pane.
    message.textContent = reason || (exited ? "" : idleLabel);
    message.hidden = !message.textContent;
    // Re-rendering IS the reset: every label and disabled flag comes back with
    // the fresh markup, so a failed start needs no cleanup of its own. The one
    // moment it must not is while a start is in flight — the offer is standing
    // there in its busy state on purpose, and it resets when the start settles.
    if (onStart && !starting) renderStartOffer();
  };

  // The resume, from the one button that offers it. It goes inert while the
  // harness comes up: a second press in flight would race two of them over one
  // worktree.
  const startAgent = async () => {
    const button = resumeButton();
    button.disabled = true;
    button.textContent = "Resuming…";
    starting = true;
    try {
      await onStart();
      starting = false;
      // The start is accepted, not finished: the bridge answers as soon as the
      // agent is queued and spawns it behind the reply. The pane is already
      // attached to the screen it will be born onto, so its first frame — or
      // its absence — is what has the last word; dropping the offer here is
      // what makes the press feel like it did something meanwhile.
      shade.hidden = true;
    } catch (e) {
      // Standing offer, plus the reason — a start that failed silently would
      // leave the human pressing a control that never explains itself.
      starting = false;
      show(exited ? "exited" : "idle", (e && e.message) || "could not start the agent");
    }
  };

  // Delegated, because the offer is re-rendered on every state change and the
  // button a press lands on is a new element each time.
  if (startOffer) {
    startOffer.onclick = (event) => {
      const button = event.target.closest("#agentResume");
      if (!button || button.disabled) return;
      startAgent();
    };
  }

  let pane = null;
  let disposed = false;
  let reconnectWatch = null;
  const mountPane = () => {
    reconnectWatch = null;
    mountAgentPane(host.querySelector("#agentpane"), target, {
      // A retained screen with no live session is a harness that ran and
      // stopped; an empty one never ran at all. The message differs; the offer
      // does not.
      onLive: (live, attached) => {
        show(live ? "live" : hasScreen(attached) ? "exited" : "idle");
      },
      onExit: (reason) => {
        if (reason === "agent_session_ended") show("exited");
      },
    }).then(
      (p) => (disposed ? p.dispose() : (pane = p)),
      (error) => {
        if (disposed) return;
        // A lost socket says nothing about this worktree — the agent may well be
        // running on the other side of it. Say the machine is out of reach and
        // attach again when it is back, instead of standing here as an empty
        // worktree until the human navigates away and comes back.
        if (isTerminalSocketLost(error)) {
          show("idle", RECONNECTING_MESSAGE);
          reconnectWatch = whenTerminalReconnects(mountPane);
          return;
        }
        // An unresolvable address (a plan whose worktree is gone, a deleted run)
        // is the empty state — the offer stands, the surface is intact.
        show("idle");
      },
    );
  };
  mountPane();
  return {
    dispose() {
      disposed = true;
      if (reconnectWatch) reconnectWatch.dispose();
      if (pane) pane.dispose();
    },
  };
}

/** Whether an attach came back with a screen the human can still read. */
function hasScreen(attached) {
  return !!(attached && attached.snapshot && attached.snapshot.length);
}
