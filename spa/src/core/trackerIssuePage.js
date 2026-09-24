// One issue's page, mounted.
//
// Cache-first like every other detail surface: what `issues.get` last answered
// is on disk under the project (core/trackerCache.js), so a revisit paints
// before the bridge is asked and the live read follows. The page writes that
// record through itself — the sync layer keeps the LIST warm, and a page is the
// only thing that knows a reader ever opened this issue.
//
// It subscribes the project (`kinds: ["issues"]`) and re-reads when an item
// names this issue. An item is content-free beyond the ids and is dropped past
// 200 of them, so a truncated item re-reads too: "refetch" is what truncation
// means.

import { messageOf } from "./text.js";
import { watchChanges } from "./changeEvents.js";
import { issuesPushKinds } from "./trackerPush.js";
import { notifyError } from "./notify.js";
import {
  advanceIssueReadThrough,
  issueAddress,
  issueRecord,
  issueRecordAt,
  issuesAddress,
  readIssueRecord,
  readIssuesRecord,
  writeIssueRecord,
} from "./trackerCache.js";
import { subscribeCache } from "./localCache.js";
import { createReadRetry } from "./transientRead.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import { trailingRead } from "./trailingRead.js";
import { onReaderReturns, readerIsHere } from "./readerPresence.js";
import { columnsOf, labelsFromText } from "./trackerModel.js";
import { timelineRows } from "./trackerTimeline.js";
import { issueLinkRows } from "./trackerLinks.js";
import { agentLabels, agentProviders, assigneeOptions, projectName, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { COMMENT_INPUT_ID, issueMissingHtml, issuePageHtml } from "./trackerIssueRender.js";
import { referenceLinks } from "./referenceTargets.js";
import { mountComposerAttachments } from "./composer.js";
import { carriesIssueAttachments } from "./issueAttachments.js";
import { carriesWatching, readThrough, watchStateOf } from "./trackerWatch.js";
import { createWatchToggle, syncWatchButton, WATCH_BUTTON_SELECTOR } from "./watchToggle.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { createThreadState, wireThreadAttachments } from "./thread.js";
import { createIssueAttachmentBodies } from "./issueAttachmentBodies.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { createUnreadMarker } from "./unreadAnchor.js";
import { issueUnreadReading, issueUnreadRules, latestIssueMark } from "./trackerUnread.js";
import { mountNewMessagesPill } from "./newMessagesPill.js";
import { scrollWithin } from "./scrollWithin.js";

/** Whether one flush of `issues` items says anything about this issue. */
const namesIssue = (items, issueId) =>
  (items || []).some((item) => item.issues && (item.issues.truncated || (item.issues.issue_ids || []).includes(issueId)));

/** Mark a routed comment after a cache paint. The row already has a stable ID
 *  for conversation action links; this also moves the issue's own scroller. */
export function focusIssueComment(host, commentId, { scroll = true } = {}) {
  if (!commentId) return false;
  const row = [...host.querySelectorAll(".issue-comment[id]")]
    .find((entry) => entry.id === `comment-${commentId}`);
  if (!row) return false;
  row.classList.add("issue-comment-target");
  if (scroll) scrollWithin(host, row, { block: "center" });
  return true;
}

export function mountIssuePage(host, options) {
  const state = {
    ...options,
    issue: null,
    rows: [],
    columns: [],
    draft: "",
    labelsDraft: "",
    busy: false,
    sending: false,
    loaded: false,
    issues: [],
    disposed: false,
    picker: null,
    // The tray's entries are the VIEW's draft, not the DOM's: this page
    // rewrites itself whole on a repaint, and an upload started before one
    // has to settle into the tray after it.
    files: [],
  };
  let pendingCommentText = null;
  const commentDraft = watchUiState(uiAddress({
    deviceId: state.deviceId,
    entityId: state.issueId,
    view: "tracker-issue",
    kind: "draft",
    sub: state.projectId,
  }), (saved) => {
    if (typeof saved?.body !== "string" || state.disposed) return;
    pendingCommentText = null;
    state.draft = saved.body;
    paint();
  }, { debounceMs: 180 });

  const groups = () => workspaceAgents(state.feed(), state.projectKey);
  const projectWorkspaces = () => (state.feed()?.workspaces || [])
    .filter((workspace) => workspace.projectKey === state.projectKey);

  /** Whether this bridge can be asked about watching at all (#65). Read once
   *  at mount: a greeting arrives before any surface paints, and a bridge does
   *  not gain a verb without a new one. */
  const offersWatch = carriesWatching(state.deviceId);

  /**
   * The watch switch, or nothing where the bridge cannot serve it.
   *
   * The rule about how the switch behaves is the shell agent's
   * (core/watchToggle.js): it moves under the finger and a refusal puts it
   * back. What is this page's is where the state comes from — the issue record
   * — and that a press repaints the BUTTON rather than the page: a full
   * repaint here would take the reader's caret out of a half-written comment.
   */
  const watch = offersWatch
    ? createWatchToggle({
      issueId: state.issueId,
      call: (method, params) => state.callRpc(method, params),
      onChange: (next) => syncWatchButton(host.querySelector(WATCH_BUTTON_SELECTOR), next),
      onFailure: () => notifyError("Could not change whether you are watching this issue"),
    })
    : null;

  /** The newest row this reader has been shown, as last told to the bridge.
   *  Held so a scroll that reaches the end twice is one call, not one a frame:
   *  the mark only ever moves forward, and re-sending the same point says
   *  nothing. */
  let markedThrough = "";
  let unreadFrom = null;
  const unreadMarker = createUnreadMarker(() => {
    unreadFrom = null;
    paint();
  }, issueUnreadRules);
  const unreadPill = mountNewMessagesPill(host, { targetSelector: ".issue-unread-line" });

  const updateUnread = () => {
    if (!offersWatch) return;
    unreadFrom = unreadMarker.update(issueUnreadReading(state.rows, state.issue?.read_through, markedThrough));
  };

  /**
   * Say how far this reader has read.
   *
   * Quiet on both sides: nothing is drawn from the answer, and a refusal is
   * swallowed rather than toasted — a read mark is housekeeping the reader did
   * not ask for, and a page that shouts about failing to keep its own notes is
   * worse than one that quietly re-sends on the next scroll.
   */
  function markRead() {
    // A page in a hidden tab or an unfocused window is open, not read. A read
    // mark is also the user being here (the bridge's user session), and a
    // window left showing this overnight must not say so each time an agent
    // comments. Coming back marks what is on screen (readerPresence.js).
    if (!offersWatch || !state.issue || !readerIsHere()) return;
    const through = readThrough(state.rows);
    if (!through || through === markedThrough) return;
    markedThrough = through;
    updateUnread();
    void Promise.resolve()
      .then(() => state.callRpc("issues.read_through", { issue_id: state.issueId, event_id: through }))
      .then((answer) => advanceIssueReadThrough(
        state.deviceId, state.projectId, state.issueId,
        latestIssueMark(through, answer?.issue?.read_through),
      ))
      .catch(() => { if (markedThrough === through) markedThrough = ""; });
  }

  /** The reader reached the end of the timeline, which is the only thing that
   *  says they have seen what is at the bottom of it. A few pixels of slack:
   *  a scroller rounded by the browser's own subpixel maths can stop a hair
   *  short of its end and never say so. */
  const AT_THE_END = 8;
  const onScroll = () => {
    if (host.scrollTop + host.clientHeight >= host.scrollHeight - AT_THE_END) markRead();
    unreadPill.update();
  };
  host.addEventListener("scroll", onScroll, { passive: true });
  const stopWaitingForReader = onReaderReturns(markRead);

  const place = () => ({ projectId: state.projectId, deviceId: state.deviceId, projectKey: state.projectKey });

  /** What this page does when a read fails because the wire went away rather
   *  than because the bridge said no: keeps what is on screen, marks when it
   *  was read, and reads again when the machine is back — without a word, as
   *  long as there is something to keep. */
  const reads = createReadRetry({
    host,
    watch: deviceWatch(state.deviceId),
    retry: () => void refresh(),
    hasContent: () => Boolean(state.issue),
  });

  // ---- painting ------------------------------------------------------------

  /** What the page last drew. A paint that would draw the same thing again
   *  is skipped outright: the feed moves every time an agent's state does,
   *  and a redraw that changes nothing on screen would still take the reader's
   *  caret and scroll with it. */
  let painted = null;
  let commentFocused = false;

  /** Where the reader is typing when the page is about to be redrawn: which
   *  field, and where the caret is in it. A push, a feed move or a write's
   *  own repaint must not take the caret away — it comes back to the same
   *  place in the field the redraw stood up. */
  const fieldSnapshot = () => {
    const active = document.activeElement;
    if (!active?.id || !host.contains(active)) return null;
    return { id: active.id, value: active.value, start: active.selectionStart, end: active.selectionEnd, scrollTop: active.scrollTop };
  };
  const restoreField = (snapshot) => {
    const field = snapshot && host.querySelector(`#${snapshot.id}`);
    if (!field) return;
    if (snapshot.id === COMMENT_INPUT_ID && pendingCommentText !== null) field.value = snapshot.value;
    field.focus({ preventScroll: true });
    if (typeof snapshot.start === "number" && field.setSelectionRange) field.setSelectionRange(snapshot.start, snapshot.end);
    field.scrollTop = snapshot.scrollTop;
  };

  const pageHtml = () => {
    if (!state.issue) return state.loaded ? issueMissingHtml() : "";
    return issuePageHtml(state.issue, {
      columns: state.columns,
      agentLabels: agentLabels(groups()),
      agentProviders: agentProviders(groups()),
      agentGroups: groups(),
      workspaces: projectWorkspaces(),
      identities: state.issue.identities || {},
      deviceId: state.deviceId,
      projectId: state.projectId,
      projectName: projectName(state.feed(), state.projectKey),
      refLinks: referenceLinks({
        place: place(),
        issues: state.issues,
        workspaces: projectWorkspaces(),
        agentGroups: groups(),
        identities: state.issue.identities || {},
      }),
      rows: state.rows,
      unreadFrom,
      links: issueLinkRows(state.issue, place(), state.feed()),
      watch: watch?.state() || null,
      draft: state.draft,
      labelsDraft: state.labelsDraft,
      busy: state.busy,
      sending: state.sending,
      // Asked at paint, never cached: a greeting lands after a page is on
      // screen, and a paperclip that waited for the next navigation would be
      // a capability nobody got the benefit of.
      attachable: carriesIssueAttachments(state.deviceId),
      hasFiles: state.files.length > 0,
    });
  };

  const paint = () => {
    if (state.disposed) return;
    const html = pageHtml();
    if (html === painted) return;
    const typing = fieldSnapshot();
    painted = html;
    host.innerHTML = html;
    if (state.issue) wire();
    unreadPill.sync();
    reads.mark(); // the host was just rewritten; the mark lives among its children
    restoreField(typing);
    if (state.commentId) {
      const found = focusIssueComment(host, state.commentId, { scroll: !commentFocused });
      if (found) commentFocused = true;
      else if (!commentFocused) host.scrollTop = 0;
    }
  };

  /** Take one cached `issues.get` record: the issue, its timeline, and the labels the
   *  rail's field opens on. A field the reader is mid-edit in is left alone —
   *  a push must not retype what somebody is typing. */
  function take(record, { keepDrafts = false, live = true } = {}) {
    state.loaded = live;
    if (!record?.issue) {
      state.issue = null;
      state.rows = [];
      return;
    }
    state.issue = record.issue;
    // Whoever mounted this page may want to say which issue is open — the
    // route names an id, but only a read knows its number and title.
    state.onIssueRead?.(record.issue);
    state.rows = timelineRows(record.timeline);
    // Read the mark while it still says where this visit began. The open
    // mark below advances it, but the held divider must not move with it.
    updateUnread();
    if (!keepDrafts) state.labelsDraft = (record.issue.labels || []).join(", ");
    // What the record says outranks anything the switch guessed, and opening
    // an issue is reading it: the mark moves on open as well as on the scroll
    // that reaches the end (#65).
    watch?.settle(watchStateOf(record.issue));
    markRead();
  }

  async function paintFromCache() {
    const [cached, list, at] = await Promise.all([
      readIssueRecord(state.deviceId, state.projectId, state.issueId),
      readIssuesRecord(state.deviceId, state.projectId),
      issueRecordAt(state.deviceId, state.projectId, state.issueId),
    ]);
    if (state.disposed) return;
    state.columns = columnsOf(list?.columns);
    takeList(list);
    if (!cached?.issue || state.issue) return;
    take(cached, { live: false });
    reads.seen(at); // this copy is as old as the cache's stamp, not as old as now
    paint();
  }

  /** The project's list, which is what a `#42` written in a comment is
   *  resolved against (#63).
   *
   *  Kept up with rather than read once: this page is often the FIRST thing
   *  opened on a device, and the list lands behind it — read once at mount, an
   *  issue number rendered as prose and stayed prose while the issue it names
   *  sat one press away. The record is the cache's own, so this costs a read
   *  when the list moves and nothing at all when it does not. */
  function takeList(list) {
    const issues = list?.issues || [];
    if (issues.length === state.issues.length && issues.every((issue, at) => issue.id === state.issues[at]?.id)) return false;
    state.issues = issues;
    return true;
  }

  const listWatcher = subscribeCache(issuesAddress(state.deviceId, state.projectId), async () => {
    const list = await readIssuesRecord(state.deviceId, state.projectId);
    if (state.disposed) return;
    if (takeList(list)) paint();
  });

  /** Detail writes are announcements, never payload delivery. Re-read the
   *  record they named and only then let it reach the renderer. */
  const issueWatcher = subscribeCache(issueAddress(state.deviceId, state.projectId, state.issueId), async () => {
    const cached = await readIssueRecord(state.deviceId, state.projectId, state.issueId);
    if (state.disposed || !cached) return;
    take(cached, { keepDrafts: Boolean(state.issue), live: true });
    reads.succeeded();
    paint();
  });

  /** Read the issue again. Agents commenting push every flush, so a read
   *  asked for while one is out waits for it and runs once after it (#126, as
   *  #119 for the list): every answer lands, and a push is never answered by a
   *  read begun before it. A write awaiting this while a read is out settles
   *  with the read after it, so the page paints what was written. */
  let issueReads = null;
  function refresh() {
    issueReads ||= trailingRead(readIssue, { generationOf: () => deviceSession(state.deviceId) });
    return issueReads();
  }

  async function readIssue() {
    if (state.disposed) return;
    try {
      const answer = await state.callRpc("issues.get", { issue_id: state.issueId });
      if (state.disposed) return;
      // The pull is a writer. The subscription above owns the read and paint.
      await writeIssueRecord(state.deviceId, state.projectId, state.issueId, issueRecord(answer.issue, answer.timeline));
    } catch (error) {
      if (state.disposed) return;
      // The wire going away is not news about this issue. With the issue on
      // screen the page keeps it and waits; with nothing on screen it waits
      // too, and says so if the read still fails once the machine is back.
      if (reads.failed(error)) return;
      state.loaded = true;
      paint();
      notifyError("Could not read this issue", messageOf(error));
    }
  }

  // ---- the writes ----------------------------------------------------------

  /** One field, one verb, one refresh. Everything the rail does goes through
   *  here, so a refusal reads the same way whichever control caused it and
   *  nothing is left half-painted. */
  async function write(method, params, whatFailed) {
    if (state.busy) return;
    state.busy = true;
    paint();
    try {
      await state.callRpc(method, params);
      await refresh();
    } catch (error) {
      if (!state.disposed) notifyError(whatFailed, messageOf(error));
    } finally {
      state.busy = false;
      paint();
    }
  }

  const toggleState = () =>
    write(
      state.issue.state === "closed" ? "issues.reopen" : "issues.close",
      { issue_id: state.issueId },
      state.issue.state === "closed" ? "Could not reopen this issue" : "Could not close this issue",
    );

  /// What a comment would carry, or null when there is nothing to send yet.
  /// A comment that is only a screenshot is a comment, so the tray can carry
  /// one on its own — but a file still going up is not one yet.
  function commentToSend() {
    if (state.sending || comments?.busy()) return null;
    const body = state.draft.trim();
    const files = comments?.attachments() || [];
    if (!body && !files.length) return null;
    return {
      issue_id: state.issueId,
      body,
      ...(files.length ? { attachments: files } : null),
    };
  }

  async function sendComment() {
    await commentDraft.flush();
    const params = commentToSend();
    if (!params) return;
    state.sending = true;
    paint();
    try {
      await state.callRpc("issues.comment", params);
      await commentDraft.write({ body: "" });
      comments?.clear();
      await refresh();
    } catch (error) {
      if (!state.disposed) notifyError("Could not add this comment", messageOf(error));
    } finally {
      state.sending = false;
      paint();
    }
  }

  function openPicker() {
    state.picker = openAssigneePicker({
      issue: state.issue,
      options: assigneeOptions(groups()),
      current: selectedOptionId(state.issue.assignee),
      catalog: state.catalog(),
      callRpc: state.callRpc,
      onAssigned: () => void refresh(),
    });
    void state.refreshCatalog?.().then((catalog) => state.picker?.setCatalog(catalog));
  }

  // ---- wiring --------------------------------------------------------------

  function wireRail() {
    host.querySelector("[data-issue-state]").onclick = toggleState;
    host.querySelector("#issue-status").onchange = (event) =>
      write("issues.update", { issue_id: state.issueId, status: event.target.value }, "Could not move this issue");
    host.querySelector("#issue-priority").onchange = (event) =>
      write("issues.update", { issue_id: state.issueId, priority: event.target.value }, "Could not set the priority");
    host.querySelector("[data-issue-assign]").onclick = openPicker;
    wireLabels();
  }

  function wireLabels() {
    const field = host.querySelector("#issue-labels");
    field.oninput = () => { state.labelsDraft = field.value; };
    field.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      void write("issues.update", { issue_id: state.issueId, labels: labelsFromText(field.value) }, "Could not set the labels");
    };
  }

  /// The comment box's tray, remounted after each repaint over the entries the
  /// view is holding — which is what lets an upload started before a repaint
  /// settle into the tray after it.
  let comments = null;
  function wireCommentAttachments(form) {
    if (!carriesIssueAttachments(state.deviceId)) {
      comments = null;
      return;
    }
    comments = mountComposerAttachments(form, {
      ids: { input: COMMENT_INPUT_ID },
      upload: (file, base64) =>
        state.callRpc("issues.attach", {
          project_id: state.projectId,
          filename: file.name,
          content_b64: base64,
        }),
      onError: (message) => notifyError("Could not attach that file", message),
      readAttachments: () => state.files,
      writeAttachments: (entries) => {
        state.files = entries;
      },
      // The send press turns on the moment a file is in the tray, and off
      // again when the last one is taken out.
      onChange: () => paint(),
    });
  }

  function wireComposer() {
    const form = host.querySelector("[data-issue-composer]");
    const field = host.querySelector(`#${COMMENT_INPUT_ID}`);
    wireCommentAttachments(form);
    field.oninput = () => {
      pendingCommentText = field.value;
      commentDraft.schedule({ body: field.value });
    };
    field.onkeydown = (event) => {
      if (event.isComposing || event.key !== "Enter" || !(event.ctrlKey || event.metaKey)) return;
      event.preventDefault();
      if (!field.value.trim() && !comments?.attachments().length) return;
      form.requestSubmit();
    };
    form.onsubmit = (event) => {
      event.preventDefault();
      void sendComment();
    };
  }

  /// The files filed with the issue (#57), loaded through the conversation's
  /// own wiring: one attachment story in this client rather than two, so the
  /// refusal and the wire-went-away deferral are already answered for.
  ///
  /// `issues.attachment` is the verb this asks for, and a bridge that does not
  /// have it yet refuses — which `wireThreadAttachments` draws as unavailable
  /// rather than as a picture that never arrives.
  ///
  /// The bytes are painted from the cache (core/issueAttachmentBodies.js), and
  /// every list on the page is wired — the body's and each comment's — so the
  /// lightbox steps through the one that was pressed.
  const attachmentState = createThreadState({ ownerId: `issue:${options.issueId}` });
  const attachmentBodies = createIssueAttachmentBodies({
    deviceId: state.deviceId,
    issueId: state.issueId,
    call: (method, params) => state.callRpc(method, params),
  });
  const wireAttachments = () =>
    wireThreadAttachments(host.querySelector(".issue-page-main"), attachmentBodies.load, attachmentState);

  function wire() {
    wireRail();
    wireComposer();
    wireAttachments();
    wireWatch();
  }

  function wireWatch() {
    const button = host.querySelector(WATCH_BUTTON_SELECTOR);
    if (button && watch) button.onclick = () => void watch.press();
  }

  // ---- lifecycle -----------------------------------------------------------

  paint();
  void paintFromCache().then(() => refresh());
  // No cadence: nothing in this client polls. An item that names this issue
  // is what re-reads it, and the pass behind that (core/cacheSync.js) is the
  // whole of the safety net.
  const watcher = watchChanges({
    refresh: () => void refresh(),
    entity: state.projectId,
    deviceId: state.deviceId,
  // Named only where the bridge carries them (core/trackerPush.js): every
  // kind in one subscribe shares that call's fate, and a refused one takes
  // this device's other subscriptions with it.
    kinds: issuesPushKinds(state.deviceId),
    mode: "realtime",
    onChanges: (items) => {
      if (namesIssue(items, state.issueId)) void refresh();
    },
  });

  return {
    /** The feed moved: the workspaces a link points at and the agents an
     *  assignee is named by may have. Nothing is re-read from the bridge. */
    feedMoved: paint,
    dispose() {
      state.disposed = true;
      commentDraft.dispose();
      host.removeEventListener("scroll", onScroll);
      stopWaitingForReader();
      watcher.dispose();
      issueWatcher?.();
      listWatcher?.();
      reads.dispose();
      unreadMarker.leave();
      unreadPill.dispose();
      attachmentBodies.dispose();
      state.picker?.close?.();
    },
  };
}
