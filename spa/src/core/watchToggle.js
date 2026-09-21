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

/** What the switch says on hover: whether the reader is watching, and how many
 *  others are. Nobody else is not a number worth showing. */
export function watchTitle({ watching = false, watchers = 0 } = {}) {
  const who = watching ? "Watching" : "Not watching";
  return watchers > 0 ? `${who} · ${watchers}` : who;
}

/**
 * A watch switch for one issue.
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
  call,
  onChange = () => {},
  onFailure = () => {},
}) {
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
        await call(watchingNow ? "issues.watch" : "issues.unwatch", { issue_id: issueId });
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
