// How much of a conversation the reader wants drawn.
//
// A conversation holds four kinds of thing: what the agent DID (tool calls,
// lifecycle events — the runs the daemon digests), what other agents said into
// it and what this agent said out to them, what this agent said here, and what
// the reader said. A branch's conversation is read to watch work happen, so all
// of it belongs on screen. A project agent's is read as correspondence — it is
// mostly other agents reporting in — and the three levels here are what lets
// one panel be both.
//
// Filtering is a RENDER-TIME reading of the cached thread: nothing here changes
// what is synced, cached or reported. Pure functions over an injected
// Web-Storage-shaped object, like every other preference this client keeps
// (core/railMode.js, core/agentSurfacesModel.js), so the module is testable
// under node and a browser that refuses storage still gets a working default.

/** The three levels, in the order the menu offers them: most detail first. */
export const DETAIL_LEVELS = ["all", "messages", "agent"];

/** What each level is called where it is chosen, and what it costs to choose
 *  it. The description names what the level SHOWS rather than what it hides —
 *  the reader is picking a view, not performing a subtraction. */
const LEVEL_COPY = {
  all: { label: "All", description: "Activity, every agent's messages, and yours" },
  messages: { label: "All messages", description: "Every agent's messages and yours" },
  agent: { label: "Agent only", description: "This agent's messages and yours" },
};

/** Where one conversation's level lives in localStorage. Dotted and `build.`
 *  prefixed, like every other key this client writes. */
export const DETAIL_LEVEL_KEY_PREFIX = "build.conversation.detail.";

export const detailLevelKey = (conversationId) => `${DETAIL_LEVEL_KEY_PREFIX}${conversationId}`;

/** The level a conversation opens at when the reader has never said. A project
 *  agent's conversation is correspondence — the work it reports on happened
 *  somewhere else, and its activity is the mechanics of relaying — so it opens
 *  on the dialogue alone. Every other rail is mounted ON the work, where the
 *  activity IS the news. */
const KIND_DEFAULT_LEVEL = { project: "agent" };
const DEFAULT_LEVEL = "all";

export function defaultDetailLevel(kind) {
  return KIND_DEFAULT_LEVEL[kind] || DEFAULT_LEVEL;
}

/** The remembered level for one conversation. Anything missing, unrecognised
 *  or unreadable is the default for this kind of rail. */
export function readDetailLevel(conversationId, kind, storage = globalThis.localStorage) {
  if (!conversationId) return defaultDetailLevel(kind);
  try {
    const stored = storage.getItem(detailLevelKey(conversationId));
    return DETAIL_LEVELS.includes(stored) ? stored : defaultDetailLevel(kind);
  } catch {
    return defaultDetailLevel(kind);
  }
}

/** Remember a level. One that does not exist is not written, so a bad value can
 *  never be read back later. */
export function writeDetailLevel(conversationId, level, storage = globalThis.localStorage) {
  if (!conversationId || !DETAIL_LEVELS.includes(level)) return;
  try {
    storage.setItem(detailLevelKey(conversationId), level);
  } catch {
    /* storage disabled — the choice lasts the session */
  }
}

/** A thread item is a message or it is activity: `type` is the whole of the
 *  difference (core/thread.js draws anything that is not a message as an event
 *  row, and folds the runs among them). */
const isMessage = (item) => item?.type === "message";

/** A message that is correspondence between agents rather than part of THIS
 *  conversation's dialogue: one that arrived from another agent, and one this
 *  agent sent out to another. Both name a conversation elsewhere, and both are
 *  drawn as such (core/thread.js `arrivedMessageHtml`, `sentMessageHtml`). */
const isBetweenAgents = (item) => Boolean(item?.data?.from_agent || item?.data?.sent_to);

/** An agent acting on a task, narrated in its own conversation
 *  (core/trackerActionLine.js). It is this agent's own act rather than
 *  correspondence with anywhere else, so it is dialogue at every level — and
 *  said here rather than left to fall out of carrying no `from_agent`, because
 *  "what this agent did" is exactly what the narrowest level is for and it
 *  should not depend on which fields the bridge happens to set. One line, so
 *  it costs the narrowed view almost nothing. */
const isOwnTaskAction = (item) => Boolean(item?.data?.task_action);

/** A tracking notice: Build saying somebody acted on a task this agent
 *  follows (core/trackerNotice.js). One line, and news about the work rather
 *  than correspondence with anywhere else, so it is dialogue at every level.
 *
 *  Said here rather than left to fall out of carrying no `from_agent`, for the
 *  same reason the action line above is: the narrowest level is for what is
 *  happening to this agent's work, and whether it survives should not depend
 *  on which fields the bridge happens to set on a notice. */
const isTrackingNotice = (item) => Boolean(item?.data?.from_build && item?.data?.from_task);

/** What each level admits. One predicate per level rather than a ladder of
 *  conditionals: a level is a way of reading the thread, and adding one should
 *  be adding a reading. */
const SHOWN_AT_LEVEL = {
  all: () => true,
  messages: isMessage,
  agent: (item) =>
    isMessage(item) && (isOwnTaskAction(item) || isTrackingNotice(item) || !isBetweenAgents(item)),
};

const shownAt = (level) => SHOWN_AT_LEVEL[level] || SHOWN_AT_LEVEL[DEFAULT_LEVEL];

/** Whether one cached item is drawn at this level. */
export function itemIsShownAt(item, level) {
  return shownAt(level)(item);
}

/** The cached thread as this level reads it. `all` is the thread itself — the
 *  view today, and no copy made for it. */
export function itemsAtDetailLevel(items, level) {
  const held = items || [];
  if (level === DEFAULT_LEVEL) return held;
  return held.filter((item) => itemIsShownAt(item, level));
}

/** The menu's rows: one per level, the standing one marked. The prefix is what
 *  keeps these ids clear of the surface kinds in the same menu
 *  (core/agentSurfacesModel.js `surfaceMenuOptions`). */
export const DETAIL_OPTION_PREFIX = "detail:";

export function detailLevelMenuOptions(level) {
  return DETAIL_LEVELS.map((name) => ({
    id: `${DETAIL_OPTION_PREFIX}${name}`,
    label: LEVEL_COPY[name].label,
    description: LEVEL_COPY[name].description,
    selected: name === level,
  }));
}

/** The menu's Detail group: the levels as a radio set under the setting's
 *  name (core/splitButton.js `groupedMenuButtonMarkup`), which is what makes
 *  a row read as a value of it. */
export function detailLevelMenuGroup(level) {
  return { id: "detail", label: "Detail", options: detailLevelMenuOptions(level) };
}

/** The level a menu id stands for, or null for an id that is not one of these
 *  rows — which is how the one menu routes two kinds of choice. */
export function detailLevelOfOptionId(optionId) {
  const id = String(optionId || "");
  if (!id.startsWith(DETAIL_OPTION_PREFIX)) return null;
  const level = id.slice(DETAIL_OPTION_PREFIX.length);
  return DETAIL_LEVELS.includes(level) ? level : null;
}

/// How far the reader has read, with the items this level hides folded back in.
///
/// `readThroughSequence` (core/thread.js) can only ask the rows that were
/// DRAWN, so an item the level hides leaves nothing to scroll past and the
/// daemon's cursor would stall under it for good: a conversation whose tail is
/// activity would keep its unread badge however far the reader scrolled.
///
/// A hidden item is read once the reader has read past where it WOULD have
/// been, which is the last drawn row above it. So from the row the viewport
/// reached, every hidden item that follows counts as read too — up to the first
/// drawn row that has NOT been reached, which is where reading honestly stops.
///
/// 0 is the daemon's word for "never read", so it absorbs nothing.
export function readThroughHiddenItems(read, items, level) {
  if (!read || level === DEFAULT_LEVEL) return read;
  let reached = read;
  for (const item of items || []) {
    const sequence = Number(item?.data?.sequence);
    if (!Number.isFinite(sequence) || sequence <= reached) continue;
    if (itemIsShownAt(item, level)) return reached;
    reached = sequence;
  }
  return reached;
}
