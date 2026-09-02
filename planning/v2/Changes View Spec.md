# Changes View Spec

Status: locked design, 2026-09-02. Gated primitives below.

## Problem

While an agent works, the Changes surface repaints on every tick the patch
text changes, and each repaint replaces the diff stack's HTML wholesale. Three
things break:

1. A diff the reader expanded collapses back to its default on the next
   repaint, because the expansion lives only in the DOM.
2. The scroll position jumps on repaint, worst when the file being read is the
   one that changed, because the scroller's content is destroyed and rebuilt.
3. There is no way to jump from a file's diff to the Files view showing that
   file.

## Design rules

- Reader-owned state lives in the controller, never only in the DOM. The
  render is a function of that state.
- The diff stack is a keyed list. A repaint patches in place; an element the
  reader is looking at survives.
- Scroll anchoring holds a node still, not a number. A tick that writes
  nothing touches nothing.
- Cross-view handoffs that the route cannot carry go through the one-shot
  `App.*OnMount` idiom, never a widened hash.

## Primitives

### A. Diff-file identity and open state

- `fileKey(file)` in `core/diff.js`, beside `patchHunks`: the path, plus the
  status to keep a rename's delete and add apart. Emitted as `data-key` on
  each `.file` element so one string names the element and the state entry.
- `expandedFiles`, a set of file keys per changeset, owned by the two
  controllers that already own `noiseExpanded` and `expandedGroups`
  (`gitPane.js` and `changesReview.js`). `diffFilesHtml` takes it beside
  `viewed`; the fold class is a function of state. The fold handlers mutate
  the set, not the class list.
- Each `.file` also carries `data-expanded` while expanded, so the patch
  guard in `domPatch.js` protects an expansion made between a press and the
  next state-driven paint.

### B. Keyed stack and scroll anchoring

- `diffRender.js` gains a sibling to `diffStackHtml` returning
  `[{ key, html }]`, the shape `changesRailEntries` already returns. Both
  controllers patch a `.dstack` container with `patchList` instead of writing
  the detail host's HTML. The container is marked `data-keyed-list`. Bars,
  triage groups, the noise group, and the comment tray sit outside or ahead
  of the keyed container.
- `paintThreadKeepingPlace` moves out of `thread.js` into
  `core/paintKeepingPlace.js` with an injected "opening" predicate and a
  policy (thread: pin to bottom; diff: anchor top). Its "a paint that wrote
  nothing touches nothing" rule stays verbatim. The thread keeps using it
  through the new module.
- Diff anchoring: before a paint, record the topmost `.file[data-key]`
  intersecting the viewport and its offset; after, find the same key and
  correct the scroll by the delta. The file above the one being read may
  grow; the one being read does not move.

### C. Jump into Files

- `App.openFileOnMount = { path, line }`, the `focusComposerOnMount` idiom:
  set by the Changes surface right before routing to the files tab, read and
  cleared by the branch view, passed to `renderFilesTab`'s existing
  `initialPath`, extended to carry a line.
- One control in each file's head beside the existing ones, carrying
  `data-open-file` and, when pressed on a hunk row, the row's new line.
  Each delegated handler claims it before the fold handling, the way the
  edit control already is.
- The Files preview marks each source row with its line and scrolls the
  requested one into view on mount.

## Out of scope

- Byte caps on the aggregate diffs.
- Rename detection.
