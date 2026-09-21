// Watching an issue, from whichever surface offers the switch.
//
// #65. The issue page's toggle (issues-spa, core/trackerIssuePage.js) and the
// inbox's Mute are the same verb on the same record, so the rule about how the
// switch behaves lives once rather than twice: the switch moves under the
// finger, and a refusal puts it back.
//
// Optimistic because the alternative is worse on a phone. A round trip on a
// dropped connection is seconds, and a switch that waits reads as broken —
// the reader presses again, and the second press is the opposite verb. So the
// state moves at once and a refusal is what corrects it, which also means the
// only lie this can tell is a short one that the next push overwrites.
//
// It holds no DOM and reaches for nothing: the surface passes `call` and gets
// told when to repaint. That is what lets the issue page and the inbox import
// the same rule without agreeing about anything else.

import { esc } from "./text.js";
import { ICON_EYE } from "./icons.js";

/** What the switch says on hover: whether the reader is watching, and how many
 *  others are. Nobody else is not a number worth showing. */
export function watchTitle({ watching = false, watchers = 0 } = {}) {
  const who = watching ? "Watching" : "Not watching";
  return watchers > 0 ? `${who} · ${watchers}` : who;
}

/**
 * What a watch is asked for, by what it is a watch OF.
 *
 * Two kinds of thing are watched and they are named differently on the wire
 * (#64): an issue by `issue_id`, a conversation by `conversation_id`. The
 * switch behaves identically for both, so the difference is one table rather
 * than two copies of the logic — and the issue form is the default, because
 * that is the shape the issue page already imports.
 */
function verbsFor({ issueId, conversationId }) {
  if (conversationId) {
    return { watch: "conversation.watch", unwatch: "conversation.unwatch", params: { conversation_id: conversationId } };
  }
  return { watch: "issues.watch", unwatch: "issues.unwatch", params: { issue_id: issueId } };
}

/**
 * A watch switch for one issue, or for one conversation.
 *
 * Pass `issueId` for an issue or `conversationId` for a conversation; the verb
 * and its parameter follow from which (`verbsFor` above).
 *
 * `watchers` is how many OTHERS watch it, so pressing counts the reader in or
 * out of the number the hover text shows. `onChange` is called with the new
 * state whenever it moves — optimistically, on settle, and on the revert — so
 * a surface repaints from one place.
 *
 * `press()` resolves when the verb has settled, refused or been ignored, which
 * is what a test awaits; nothing rejects, because a refused watch is not the
 * caller's error to handle — it is `onFailure`'s, and the switch has already
 * put itself back.
 */
export function createWatchToggle({
  watching = false,
  watchers = 0,
  issueId,
  conversationId,
  call,
  onChange = () => {},
  onFailure = () => {},
}) {
  const asked = verbsFor({ issueId, conversationId });
  let state = { watching: Boolean(watching), watchers: Number(watchers) || 0, pending: false };

  const moveTo = (next) => {
    state = { ...state, ...next };
    onChange({ ...state });
  };

  return {
    state: () => ({ ...state }),
    title: () => watchTitle(state),

    /** The reader pressed it. A press while one is in flight is ignored rather
     *  than queued: two presses would send opposite verbs and land on whichever
     *  answered last. */
    async press() {
      if (state.pending) return;
      const before = { watching: state.watching, watchers: state.watchers };
      const watchingNow = !before.watching;
      moveTo({
        watching: watchingNow,
        // The reader joins or leaves the count their own press changes.
        watchers: Math.max(0, before.watchers + (watchingNow ? 1 : -1)),
        pending: true,
      });
      try {
        await call(watchingNow ? asked.watch : asked.unwatch, asked.params);
        moveTo({ pending: false });
      } catch (error) {
        moveTo({ ...before, pending: false });
        onFailure(error);
      }
    },

    /** What the bridge says, which outranks anything this guessed. The push
     *  carries the truth; a guess that disagrees with it was wrong. */
    settle({ watching: isWatching, watchers: count }) {
      moveTo({
        watching: Boolean(isWatching),
        watchers: Number(count) || 0,
        pending: false,
      });
    },
  };
}

/// What the control wears and how it is found. Exported so the rail's paint and
/// a test name the same thing.
export const WATCH_BUTTON_CLASS = "rail-watch";
export const WATCH_BUTTON_SELECTOR = `.${WATCH_BUTTON_CLASS}`;

/**
 * The watch control, as markup.
 *
 * An eye rather than a word: it stands in a row of icon buttons beside the
 * remove button, and a word there would be the only one. `aria-pressed` is
 * what says whether it is on — the icon alone is not an answer to a reader who
 * cannot see it — and the title carries the count, which is the whole reason
 * the control is worth hovering.
 *
 * Draws the same states `syncWatchButton` writes, `pending` included: a head
 * rebuilt mid-flight must not come back offering a press the switch will
 * ignore.
 */
export function watchButtonHtml(state = {}) {
  const label = esc(watchTitle(state));
  const on = Boolean(state.watching);
  return `<button type="button" class="iconbtn ${WATCH_BUTTON_CLASS}${on ? " watching" : ""}"
    aria-pressed="${on}" title="${label}" aria-label="${label}"${state.pending ? " disabled" : ""}>${ICON_EYE}</button>`;
}

/** …and the same state written onto a button already standing, so a press does
 *  not have to rebuild the head it lives in. */
export function syncWatchButton(button, state = {}) {
  if (!button) return;
  const label = watchTitle(state);
  button.setAttribute("aria-pressed", String(Boolean(state.watching)));
  button.setAttribute("title", label);
  button.setAttribute("aria-label", label);
  button.classList.toggle("watching", Boolean(state.watching));
  // A verb in flight is not the reader's to press again (`press` ignores it
  // anyway); saying so is what keeps the control honest about why.
  button.toggleAttribute("disabled", Boolean(state.pending));
}
