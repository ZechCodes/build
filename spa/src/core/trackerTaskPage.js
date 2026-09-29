// One task's page, mounted.
//
// Cache-first like every other detail surface: what `tasks.get` last answered
// is on disk under the project (core/trackerCache.js), so a revisit paints
// before the bridge is asked and the live read follows. The page writes that
// record through itself — the sync layer keeps the LIST warm, and a page is the
// only thing that knows a reader ever opened this task.
//
// It subscribes the project (`kinds: ["tasks"]`) and re-reads when an item
// names this task. An item is content-free beyond the ids and is dropped past
// 200 of them, so a truncated item re-reads too: "refetch" is what truncation
// means.

import { messageOf } from "./text.js";
import { watchChanges } from "./changeEvents.js";
import { tasksPushKinds } from "./trackerPush.js";
import { notifyError } from "./notify.js";
import {
  advanceTaskReadThrough,
  taskAddress,
  taskRecord,
  taskRecordAt,
  readTaskRecord,
  readTasksRecord,
  writeTaskRecord,
} from "./trackerCache.js";
import { subscribeCache } from "./localCache.js";
import { createReadRetry } from "./transientRead.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import { trailingRead } from "./trailingRead.js";
import { onReaderReturns, readerIsHere } from "./readerPresence.js";
import { columnsOf, labelsFromText } from "./trackerModel.js";
import { timelineRows } from "./trackerTimeline.js";
import { taskLinkRows } from "./trackerLinks.js";
import { agentLabels, agentProviders, assigneeOptions, projectName, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import {
  canComment,
  COMMENT_BOX_ATTACHABLE_CLASSES,
  commentAttachHtml,
  commentTrayHtml,
  COMMENT_INPUT_ID,
  commentSendLabel,
  TASK_PAGE_FRAME,
  taskMissingHtml,
  taskPageParts,
} from "./trackerTaskRender.js";
import { patchParts } from "./partPatch.js";
import { subscribeReferenceIndex } from "./referenceIndex.js";
import { mountComposerAttachments } from "./composer.js";
import { carriesTaskAttachments } from "./taskAttachments.js";
import { carriesWatching, readThrough, watchStateOf } from "./trackerWatch.js";
import { createWatchToggle, syncWatchButton, WATCH_BUTTON_SELECTOR } from "./watchToggle.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { createThreadState, wireThreadAttachments } from "./thread.js";
import { createTaskAttachmentBodies } from "./taskAttachmentBodies.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { createUnreadMarker } from "./unreadAnchor.js";
import { taskUnreadReading, taskUnreadRules, latestTaskMark } from "./trackerUnread.js";
import { mountNewMessagesPill } from "./newMessagesPill.js";
import { scrollWithin } from "./scrollWithin.js";

/** Whether one flush of `tasks` items says anything about this task. */
const namesTask = (items, taskId) =>
  (items || []).some((item) => item.tasks && (item.tasks.truncated || (item.tasks.task_ids || []).includes(taskId)));

/** Mark a routed comment after a cache paint. The row already has a stable ID
 *  for conversation action links; this also moves the task's own scroller. */
export function focusTaskComment(host, commentId, { scroll = true } = {}) {
  if (!commentId) return false;
  const row = [...host.querySelectorAll(".task-comment[id]")]
    .find((entry) => entry.id === `comment-${commentId}`);
  if (!row) return false;
  row.classList.add("task-comment-target");
  if (scroll) scrollWithin(host, row, { block: "center" });
  return true;
}

export function mountTaskPage(host, options) {
  const state = {
    ...options,
    task: null,
    rows: [],
    columns: [],
    draft: "",
    labelsDraft: "",
    busy: false,
    sending: false,
    loaded: false,
    disposed: false,
    picker: null,
    // The tray's entries are the VIEW's draft, not the DOM's: a bridge that
    // gains attachments stands up a new box, and an upload started before
    // that has to settle into the tray after it.
    files: [],
  };
  const commentDraft = watchUiState(uiAddress({
    deviceId: state.deviceId,
    entityId: state.taskId,
    view: "tracker-task",
    kind: "draft",
    sub: state.projectId,
  }), (saved) => {
    if (typeof saved?.body !== "string" || state.disposed) return;
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
   * back. What is this page's is where the state comes from — the task record
   * — and that a press repaints the BUTTON rather than the page: a full
   * repaint here would take the reader's caret out of a half-written comment.
   */
  const watch = offersWatch
    ? createWatchToggle({
      taskId: state.taskId,
      call: (method, params) => state.callRpc(method, params),
      onChange: (next) => syncWatchButton(host.querySelector(WATCH_BUTTON_SELECTOR), next),
      onFailure: () => notifyError("Could not change whether you are watching this task"),
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
  }, taskUnreadRules);
  const unreadPill = mountNewMessagesPill(host, { targetSelector: ".task-unread-line" });

  const updateUnread = () => {
    if (!offersWatch) return;
    unreadFrom = unreadMarker.update(taskUnreadReading(state.rows, state.task?.read_through, markedThrough));
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
    if (!offersWatch || !state.task || !readerIsHere()) return;
    const through = readThrough(state.rows);
    if (!through || through === markedThrough) return;
    markedThrough = through;
    updateUnread();
    void Promise.resolve()
      .then(() => state.callRpc("tasks.read_through", { task_id: state.taskId, event_id: through }))
      .then((answer) => advanceTaskReadThrough(
        state.deviceId, state.projectId, state.taskId,
        latestTaskMark(through, answer?.task?.read_through),
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
    hasContent: () => Boolean(state.task),
  });

  // ---- painting ------------------------------------------------------------

  /** What the page last drew, part by part (core/partPatch.js). A repaint
   *  replaces only the parts whose HTML changed: the feed moves every time an
   *  agent's state does, a draft write comes back through the cache on every
   *  pause in typing, and a part stood up again under the reader takes their
   *  caret, and the height of every picture in it, with it (#153). */
  const held = { main: new Map(), rail: new Map(), timeline: new Map() };
  /** Whether the host holds the task page's frame, or something else (the
   *  missing notice, or nothing yet). */
  let framed = false;
  let unframed = null;
  let commentFocused = false;
  let openedAtTop = false;

  /** Where the reader is typing when a part is about to be redrawn: which
   *  field, and where the caret is in it. The rail is a part like any other,
   *  and a push or a write's own repaint must not take the caret out of its
   *  labels field — it comes back to the same place in the field the redraw
   *  stood up. The comment box is never redrawn under the reader. */
  const fieldSnapshot = () => {
    const active = document.activeElement;
    if (!active?.id || !host.contains(active)) return null;
    return { element: active, id: active.id, start: active.selectionStart, end: active.selectionEnd, scrollTop: active.scrollTop };
  };
  const restoreField = (snapshot) => {
    if (!snapshot || snapshot.element.isConnected) return;
    const field = host.querySelector(`#${snapshot.id}`);
    if (!field) return;
    field.focus({ preventScroll: true });
    if (typeof snapshot.start === "number" && field.setSelectionRange) field.setSelectionRange(snapshot.start, snapshot.end);
    field.scrollTop = snapshot.scrollTop;
  };

  const pageContext = () => ({
    columns: state.columns,
    agentLabels: agentLabels(groups()),
    agentProviders: agentProviders(groups()),
    agentGroups: groups(),
    workspaces: projectWorkspaces(),
    identities: state.task.identities || {},
    deviceId: state.deviceId,
    projectId: state.projectId,
    projectName: projectName(state.feed(), state.projectKey),
    rows: state.rows,
    unreadFrom,
    links: taskLinkRows(state.task, place(), state.feed()),
    watch: watch?.state() || null,
    draft: state.draft,
    labelsDraft: state.labelsDraft,
    busy: state.busy,
    sending: state.sending,
    // Asked at paint, never cached: a greeting lands after a page is on
    // screen, and a paperclip that waited for the next navigation would be
    // a capability nobody got the benefit of.
    attachable: carriesTaskAttachments(state.deviceId),
    hasFiles: state.files.length > 0,
  });

  /** Anything but a task: the missing notice, or nothing while the first
   *  read is out. */
  const paintUnframed = (html) => {
    if (!framed && html === unframed) return;
    framed = false;
    unframed = html;
    Object.values(held).forEach((parts) => parts.clear());
    host.innerHTML = html;
    unreadPill.sync();
    reads.mark(); // the host was just rewritten; the mark lives among its children
  };

  const frame = () => {
    if (framed) return;
    framed = true;
    unframed = null;
    Object.values(held).forEach((parts) => parts.clear());
    host.innerHTML = TASK_PAGE_FRAME;
    reads.mark();
  };

  const paint = () => {
    if (state.disposed) return;
    if (!state.task) return paintUnframed(state.loaded ? taskMissingHtml() : "");
    const parts = taskPageParts(state.task, pageContext());
    const typing = fieldSnapshot();
    frame();
    const main = host.querySelector(".task-page-main");
    const painted = new Set(patchParts(main, held.main, parts.main));
    patchParts(main.parentElement, held.rail, parts.rail, { after: main }).forEach((name) => painted.add(name));
    if (patchTimeline(parts.timeline, painted.has("timeline"))) painted.add("timeline");
    wire(painted);
    syncCommentAttachments();
    syncCommentBox();
    if (painted.has("timeline")) unreadPill.sync();
    restoreField(typing);
    markRoutedComment();
  };

  /** The timeline's rows, patched inside the list the `timeline` part stood
   *  up; a list just stood up holds none yet. Answers whether any row was
   *  painted or taken out, which is what the wiring and the unread pill
   *  listen for: a removed row can take the unread divider with it. */
  function patchTimeline(rows, newList) {
    if (newList) held.timeline.clear();
    const list = host.querySelector(".task-timeline");
    return Boolean(list) && patchParts(list, held.timeline, rows).length > 0;
  }

  /** The comment box's live state, set on the one box this mount made: its
   *  draft, whether it is sending, and whether it can be sent. A value is
   *  only written when it differs — the reader's own keystrokes come back
   *  through the cache as the value already in the box — so a restored or
   *  cleared draft lands and a typed one is never touched. */
  function syncCommentBox() {
    const field = host.querySelector(`#${COMMENT_INPUT_ID}`);
    const send = host.querySelector('[data-task-composer] button[type="submit"]');
    if (!field || !send) return;
    if (field.value !== state.draft) takeDraftInto(field, state.draft);
    field.disabled = state.sending;
    send.disabled = !canComment(state.draft, state.sending, state.files.length > 0);
    send.textContent = commentSendLabel(state.sending);
  }

  /** Put a draft that came from elsewhere — another tab writing the same
   *  record, a restore, a send clearing it — into the box without moving the
   *  reader: assigning a value throws the caret to its end and the field's
   *  own scroll to wherever that is, so both are put back, the selection
   *  clamped to what the new text holds. */
  function takeDraftInto(field, draft) {
    const { selectionStart: start, selectionEnd: end, scrollTop } = field;
    field.value = draft;
    if (document.activeElement === field && typeof start === "number") {
      field.setSelectionRange(Math.min(start, draft.length), Math.min(end, draft.length));
    }
    field.scrollTop = scrollTop;
  }

  /** The paperclip, the picker, the drop mask and the tray, hung around the
   *  textarea on screen when this bridge can carry files and taken off it when
   *  it cannot. A greeting can land after the page painted from the cache, and
   *  the reader may be typing by then: the textarea is one node per mount, so
   *  its caret and its undo history stay with it (#153).
   *
   *  Controls taken off are kept, not dropped: their tray and its uploads are
   *  wired to this textarea, and they go back on as they were. While they are
   *  held, the paste and drop wired with them take nothing. */
  let heldControls = null;
  function syncCommentAttachments() {
    const frame = host.querySelector(".task-comment-field");
    const attachable = carriesTaskAttachments(state.deviceId);
    if (!frame || frame.classList.contains("attachable") === attachable) return;
    COMMENT_BOX_ATTACHABLE_CLASSES.forEach((name) => frame.classList.toggle(name, attachable));
    if (!attachable) {
      heldControls = { tray: frame.previousElementSibling, extras: [...frame.children].filter((child) => child.id !== COMMENT_INPUT_ID), comments };
      heldControls.tray.remove();
      heldControls.extras.forEach((node) => node.remove());
      comments = null;
      return;
    }
    if (heldControls) {
      frame.before(heldControls.tray);
      frame.append(...heldControls.extras);
      comments = heldControls.comments;
      heldControls = null;
      return;
    }
    frame.insertAdjacentHTML("beforebegin", commentTrayHtml());
    frame.insertAdjacentHTML("beforeend", commentAttachHtml());
    wireCommentAttachments(frame.closest("[data-task-composer]"));
  }

  /** The comment a link routed to: marked on every paint that stood its row
   *  up, scrolled to once, and the page opened at the top — once — while the
   *  row is not there yet. */
  function markRoutedComment() {
    if (!state.commentId) return;
    const found = focusTaskComment(host, state.commentId, { scroll: !commentFocused });
    if (found) commentFocused = true;
    else if (!commentFocused && !openedAtTop) host.scrollTop = 0;
    openedAtTop = true;
  }

  /** Take one cached `tasks.get` record: the task, its timeline, and the labels the
   *  rail's field opens on. A field the reader is mid-edit in is left alone —
   *  a push must not retype what somebody is typing. */
  function take(record, { keepDrafts = false, live = true } = {}) {
    state.loaded = live;
    if (!record?.task) {
      state.task = null;
      state.rows = [];
      return;
    }
    state.task = record.task;
    // Whoever mounted this page may want to say which task is open — the
    // route names an id, but only a read knows its number and title.
    state.onTaskRead?.(record.task);
    state.rows = timelineRows(record.timeline);
    // Read the mark while it still says where this visit began. The open
    // mark below advances it, but the held divider must not move with it.
    updateUnread();
    if (!keepDrafts) state.labelsDraft = (record.task.labels || []).join(", ");
    // What the record says outranks anything the switch guessed, and opening
    // a task is reading it: the mark moves on open as well as on the scroll
    // that reaches the end (#65).
    watch?.settle(watchStateOf(record.task));
    markRead();
  }

  async function paintFromCache() {
    const [cached, list, at] = await Promise.all([
      readTaskRecord(state.deviceId, state.projectId, state.taskId),
      readTasksRecord(state.deviceId, state.projectId),
      taskRecordAt(state.deviceId, state.projectId, state.taskId),
    ]);
    if (state.disposed) return;
    state.columns = columnsOf(list?.columns);
    if (!cached?.task || state.task) return;
    take(cached, { live: false });
    reads.seen(at); // this copy is as old as the cache's stamp, not as old as now
    paint();
  }

  /** What a reference written on this page names is the shared index's
   *  answer (core/referenceIndex.js, #229): the project's task list, the
   *  feed's workspaces and agents, every project's. This page is often the
   *  FIRST thing opened on a device and those land behind it, so a change that
   *  could move an answer repaints — prose becomes a link while the reader
   *  looks. The paint is part-patched, so nothing that did not move is touched. */
  const referencesWatcher = subscribeReferenceIndex(() => paint());

  /** Detail writes are announcements, never payload delivery. Re-read the
   *  record they named and only then let it reach the renderer. */
  const taskWatcher = subscribeCache(taskAddress(state.deviceId, state.projectId, state.taskId), async () => {
    const cached = await readTaskRecord(state.deviceId, state.projectId, state.taskId);
    if (state.disposed || !cached) return;
    take(cached, { keepDrafts: Boolean(state.task), live: true });
    reads.succeeded();
    paint();
  });

  /** Read the task again. Agents commenting push every flush, so a read
   *  asked for while one is out waits for it and runs once after it (#126, as
   *  #119 for the list): every answer lands, and a push is never answered by a
   *  read begun before it. A write awaiting this while a read is out settles
   *  with the read after it, so the page paints what was written. */
  let taskReads = null;
  function refresh() {
    taskReads ||= trailingRead(readTask, { generationOf: () => deviceSession(state.deviceId) });
    return taskReads();
  }

  async function readTask() {
    if (state.disposed) return;
    try {
      const answer = await state.callRpc("tasks.get", { task_id: state.taskId });
      if (state.disposed) return;
      // The pull is a writer. The subscription above owns the read and paint.
      await writeTaskRecord(state.deviceId, state.projectId, state.taskId, taskRecord(answer.task, answer.timeline));
    } catch (error) {
      if (state.disposed) return;
      // The wire going away is not news about this task. With the task on
      // screen the page keeps it and waits; with nothing on screen it waits
      // too, and says so if the read still fails once the machine is back.
      if (reads.failed(error)) return;
      state.loaded = true;
      paint();
      notifyError("Could not read this task", messageOf(error));
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
      state.task.state === "closed" ? "tasks.reopen" : "tasks.close",
      { task_id: state.taskId },
      state.task.state === "closed" ? "Could not reopen this task" : "Could not close this task",
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
      task_id: state.taskId,
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
      await state.callRpc("tasks.comment", params);
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
      task: state.task,
      options: assigneeOptions(groups()),
      current: selectedOptionId(state.task.assignee),
      catalog: state.catalog(),
      callRpc: state.callRpc,
      onAssigned: () => void refresh(),
    });
    void state.refreshCatalog?.().then((catalog) => state.picker?.setCatalog(catalog));
  }

  // ---- wiring --------------------------------------------------------------

  function wireRail() {
    host.querySelector("[data-task-state]").onclick = toggleState;
    host.querySelector("#task-status").onchange = (event) =>
      write("tasks.update", { task_id: state.taskId, status: event.target.value }, "Could not move this task");
    host.querySelector("#task-priority").onchange = (event) =>
      write("tasks.update", { task_id: state.taskId, priority: event.target.value }, "Could not set the priority");
    host.querySelector("[data-task-assign]").onclick = openPicker;
    wireLabels();
  }

  function wireLabels() {
    const field = host.querySelector("#task-labels");
    field.oninput = () => { state.labelsDraft = field.value; };
    field.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      void write("tasks.update", { task_id: state.taskId, labels: labelsFromText(field.value) }, "Could not set the labels");
    };
  }

  /// The comment box's tray, mounted with the box over the entries the view is
  /// holding — which is what lets an upload started before a new box was stood
  /// up settle into the tray after it.
  let comments = null;
  function wireCommentAttachments(form) {
    if (!carriesTaskAttachments(state.deviceId)) {
      comments = null;
      return;
    }
    comments = mountComposerAttachments(form, {
      ids: { input: COMMENT_INPUT_ID },
      upload: (file, base64) =>
        state.callRpc("tasks.attach", {
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
      // Its paste and drop stay on the box and the form, which outlive the
      // controls: while those are held off, the box is a plain one.
      accepting: () => !heldControls,
    });
  }

  function wireComposer() {
    const form = host.querySelector("[data-task-composer]");
    heldControls = null;
    const field = host.querySelector(`#${COMMENT_INPUT_ID}`);
    wireCommentAttachments(form);
    field.oninput = () => {
      state.draft = field.value;
      commentDraft.schedule({ body: field.value });
      syncCommentBox();
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

  /// The files filed with the task (#57), loaded through the conversation's
  /// own wiring: one attachment story in this client rather than two, so the
  /// refusal and the wire-went-away deferral are already answered for.
  ///
  /// `tasks.attachment` is the verb this asks for, and a bridge that does not
  /// have it yet refuses — which `wireThreadAttachments` draws as unavailable
  /// rather than as a picture that never arrives.
  ///
  /// The bytes are painted from the cache (core/taskAttachmentBodies.js), and
  /// every list on the page is wired — the body's and each comment's — so the
  /// lightbox steps through the one that was pressed.
  const attachmentState = createThreadState({ ownerId: `task:${options.taskId}` });
  const attachmentBodies = createTaskAttachmentBodies({
    deviceId: state.deviceId,
    taskId: state.taskId,
    call: (...args) => state.callRpc(...args),
  });
  const wireAttachments = () =>
    wireThreadAttachments(host.querySelector(".task-page-main"), async (path) => ({
      ...await attachmentBodies.load(path),
      onBlob: () => void attachmentBodies.forget(path),
    }), attachmentState);

  /** Wire what a paint stood up, and nothing it left alone. */
  function wire(painted) {
    if (painted.has("head")) wireWatch();
    if (painted.has("rail")) wireRail();
    if (painted.has("composer")) wireComposer();
    if (["body", "attachments", "timeline"].some((part) => painted.has(part))) wireAttachments();
  }

  function wireWatch() {
    const button = host.querySelector(WATCH_BUTTON_SELECTOR);
    if (button && watch) button.onclick = () => void watch.press();
  }

  // ---- lifecycle -----------------------------------------------------------

  paint();
  void paintFromCache().then(() => refresh());
  // No cadence: nothing in this client polls. An item that names this task
  // is what re-reads it, and the pass behind that (core/cacheSync.js) is the
  // whole of the safety net.
  const watcher = watchChanges({
    refresh: () => void refresh(),
    entity: state.projectId,
    deviceId: state.deviceId,
  // Named only where the bridge carries them (core/trackerPush.js): every
  // kind in one subscribe shares that call's fate, and a refused one takes
  // this device's other subscriptions with it.
    kinds: tasksPushKinds(state.deviceId),
    mode: "realtime",
    onChanges: (items) => {
      if (namesTask(items, state.taskId)) void refresh();
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
      taskWatcher?.();
      referencesWatcher();
      reads.dispose();
      unreadMarker.leave();
      unreadPill.dispose();
      attachmentBodies.dispose();
      state.picker?.close?.();
    },
  };
}
