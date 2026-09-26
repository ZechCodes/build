// Pure text helpers shared by every surface.

// Quotes are escaped too: esc() output is interpolated into attribute values
// (data-dir="...", data-tab="...") where an unescaped quote from an untrusted
// name (e.g. a repo filename) would inject live attributes.
export const esc = (value) =>
  (value ?? "")
    .toString()
    .replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** What a refusal says on the surface that asked. A thrown Error carries the
 *  words; anything else a call rejected with is shown as it reads. */
export const messageOf = (error) => (error instanceof Error ? error.message : String(error));

/** What the account says about the one machine it had and lost: honest about
 *  WHEN it went unreachable (a moving "reconnecting…" claim reads as a lie while
 *  nothing is happening) and calm about what resumes automatically. */
export function deviceUnreachableText(name, sinceMs) {
  return `${name || "Your device"} unreachable since ${clockTime(sinceMs)} — tasks will resume when it reconnects.`;
}

/** A moment as a clock reads it. The one format every sentence here puts a
 *  time in, so "since 4:42 pm" and "last read at 4:42 pm" are the same time
 *  said the same way. */
export const clockTime = (whenMs) =>
  new Date(whenMs).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** What a surface says over a copy it is still showing while the machine that
 *  answered for it is being reconnected to: when this copy was read, and that
 *  something is being done about it. Deliberately not an error — the copy on
 *  screen is real, it is simply not the newest thing there is. A surface with
 *  no read behind it yet says only what is happening. */
export const lastReadText = (sinceMs) =>
  sinceMs ? `Last read at ${clockTime(sinceMs)}, reconnecting` : "Reconnecting";

/** What a work surface says when the machine its link names cannot answer —
 *  gone offline, or never opened on this client: there is nothing to read and
 *  nothing to write until that machine is back. */
export const deviceOfflineText = (name) =>
  `${name || "That device"} isn't connected, so this can't be opened right now.`;

/** What a work surface says when the machine it is about goes while it is open:
 *  it keeps what it read, so the only thing missing is whose state that is. */
export const deviceFrozenText = (name) =>
  `${name || "That device"} isn't connected — this is what it last said.`;

/** What a work surface says when the machine its link names speaks a newer
 *  Build API than this tab: the machine is answering, but nothing here knows
 *  the shape of its answers, so a reload is the whole of the fix. */
export const deviceAppBehindText = (name, version) =>
  `${name || "That device"} speaks Build API ${version || "unknown"}, which is newer than this app — reload to open it.`;

/** And the other way round: the machine's bridge is older than every API this
 *  app speaks, so the update is on that machine. */
export const deviceBridgeBehindText = (name, version) =>
  `${name || "That device"} speaks Build API ${version || "unknown"}, which is older than this app — update its bridge to open it.`;

/** What a workspace whose checkout the bridge could not build says, wherever it
 *  is listed — the rail row and the toolbar's switcher both. The reason's first
 *  line only: a list row is one line, and the whole of a git error belongs on
 *  the surface that offers the retry. */
export const workspaceFailedText = (reason) => {
  const first = String(reason || "").split("\n").map((line) => line.trim()).find(Boolean);
  return first ? `Failed: ${first}` : "Failed";
};

/** The short mark a control whose machine cannot answer wears: the title on a
 *  greyed row, the reason a shut menu item gives, and the words a call to that
 *  machine is refused with. A label, not a sentence — the sentences above are
 *  for surfaces with room for one. */
export const deviceOfflineMark = "Device offline";

/** And the mark the account's own control wears while none of its machines can
 *  answer at all. The app keeps the page — it is standing on its cache — so
 *  this is what says that what is on screen is the last thing Build saw. */
export const nothingAnswersMark = "No device is answering — showing what Build last saw";

/** And the marks for the two version gaps: the machine is answering, so saying
 *  it is offline would be a lie. Which side is out of date is the whole of what
 *  a control has room to say; the sentences above say what to do about it. */
export const appBehindMark = "App is out of date";
export const bridgeBehindMark = "Bridge is out of date";

/** What a machine the browser could not open a direct connection to says, in
 *  the one sentence a surface has room for (spec rule 3). Its bridge is up and
 *  the account lists it online — it is the connection between here and there
 *  that could not be made — so it never says the machine is offline. `why` is
 *  the plain-words half, one per reason (core/deviceAway.js). */
export const deviceBlockedText = (name, why) =>
  `${name || "This machine"}'s direct connection could not be made: ${why}.`;

/** The short mark a control shut for want of a direct connection wears, and the
 *  words a call to that machine is refused with. */
export const deviceBlockedMark = "Device not reachable";

/** And the one word its greyed rows carry. Not "offline": the machine is there,
 *  this browser cannot get to it. */
export const deviceBlockedWord = "blocked";

/** The one word a greyed row and the device picker both wear to say a machine
 *  is not here. Lower case: it is a mark on something else, never a sentence of
 *  its own. */
export const deviceOfflineWord = "offline";

/** And the words for the two version gaps, in what a row has room for: the one
 *  thing that would make that machine readable again. */
export const appBehindWord = "reload";
export const bridgeBehindWord = "update";

/** What the waiting screen is waiting for. An account with several machines
 *  lists them all under this heading, and any one of them hands the app back —
 *  so naming "your device" over that list promises one of them in particular. */
export const waitingForDeviceText = (deviceCount) =>
  deviceCount > 1 ? "Waiting for a device" : "Waiting for your device";

/** What it says where nothing is reachable: no device to name, and no time that
 *  would mean anything, so it says what is true and what happens. */
export const allDevicesOfflineText = () =>
  "All devices are offline — tasks will resume when one reconnects.";

/** What the waiting screen says when every machine the account lists is online
 *  and none of them could be reached directly (spec rule 3). Nothing is
 *  offline, so nothing is said to be: what failed is the connection, and each
 *  machine's own row offers the retry. */
export const devicesBlockedText = () =>
  "Your machines are online but none could be reached directly — retry one of them below.";

/** What it says while the account calls a machine online and this client has
 *  not got through to it yet — a boot still in the handshake, or one that
 *  failed. Nothing has gone offline, so nothing is named as having gone. */
export const devicesNotReachedYetText = () =>
  "Your devices report online, but Build could not reach one yet. It will keep trying automatically — no need to refresh.";

/** What a pin control says about the thing it docks. The words are the gesture,
 *  never the state — "Unpin the inbox" is what pressing it does, not where the
 *  inbox is — and the subject is the thing being pinned, in the reader's words:
 *  the inbox, the conversation. */
export const pinText = (pinned, subject) => `${pinned ? "Unpin" : "Pin"} the ${subject}`;

/** What a checkout's two faces are called on the rail that switches between
 *  them. The rail draws an icon and nothing else, so these words ARE the
 *  control's name: they are its tooltip and what a screen reader reads out.
 *  One says what moved, the other says what is there. */
export const changesTabLabel = "Changes";
export const filesTabLabel = "Files";
/** A workspace's third face: the issues its own agents are holding (#29). Not
 *  the project's whole tracker — the word is the same because it is the same
 *  tracker, narrowed to the agents standing here. */
export const issuesTabLabel = "Issues";
/** The cog at the foot of a workspace's rail: what the workspace is called,
 *  what its agents start on, and the one way to delete it. */
export const workspaceSettingsLabel = "Workspace settings";

/** The control at the foot of the same rail, which folds the checkout's list
 *  column (the file tree, the commit rail) away and brings it back. Icon-only
 *  too, so these are its tooltip and its name, and they say what a press does. */
export const sidebarToggleLabel = (collapsed) => (collapsed ? "Expand sidebar" : "Collapse sidebar");

/** What a two-column pane's drawer trigger offers while nothing is open — the
 *  gesture, not the state, because on a phone the list it names is behind the
 *  trigger itself. */
export const pickAChangesetText = "Pick a commit";
export const pickAFileText = "Choose a file";

/** Human-scale age: <60s "just now", <1h "Nm ago", <1d "Nh ago", else "Nd ago". */
export function humanAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
