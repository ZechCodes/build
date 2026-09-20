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
import { notifyError } from "./notify.js";
import { issueRecord, readIssueRecord, readIssuesRecord, writeIssueRecord } from "./trackerCache.js";
import { columnsOf } from "./trackerModel.js";
import { timelineRows } from "./trackerTimeline.js";
import { issueLinkRows } from "./trackerLinks.js";
import { agentLabels, assigneeOptions, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { issueMissingHtml, issuePageHtml } from "./trackerIssueRender.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { labelsFromText } from "./trackerCreate.js";

/** Whether one flush of `issues` items says anything about this issue. */
const namesIssue = (items, issueId) =>
  (items || []).some((item) => item.issues && (item.issues.truncated || (item.issues.issue_ids || []).includes(issueId)));

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
    disposed: false,
    picker: null,
  };

  const groups = () => workspaceAgents(state.feed(), state.projectKey);
  const place = () => ({ projectId: state.projectId, deviceId: state.deviceId, projectKey: state.projectKey });

  // ---- painting ------------------------------------------------------------

  const paint = () => {
    if (state.disposed) return;
    if (!state.issue) {
      host.innerHTML = state.loaded ? issueMissingHtml() : "";
      return;
    }
    host.innerHTML = issuePageHtml(state.issue, {
      columns: state.columns,
      agentLabels: agentLabels(groups()),
      rows: state.rows,
      links: issueLinkRows(state.issue, place(), state.feed()),
      draft: state.draft,
      labelsDraft: state.labelsDraft,
      busy: state.busy,
      sending: state.sending,
    });
    wire();
  };

  /** Take one `issues.get` answer: the issue, its timeline, and the labels the
   *  rail's field opens on. A field the reader is mid-edit in is left alone —
   *  a push must not retype what somebody is typing. */
  function take(answer, { keepDrafts = false } = {}) {
    if (!answer?.issue) return;
    state.issue = answer.issue;
    state.rows = timelineRows(answer.timeline);
    if (!keepDrafts) state.labelsDraft = (answer.issue.labels || []).join(", ");
    state.loaded = true;
  }

  async function paintFromCache() {
    const [held, list] = await Promise.all([
      readIssueRecord(state.deviceId, state.projectId, state.issueId),
      readIssuesRecord(state.deviceId, state.projectId),
    ]);
    if (state.disposed) return;
    state.columns = columnsOf(list?.columns);
    if (!held?.issue || state.issue) return;
    take(held);
    state.loaded = false; // a cached paint is not an answer about what exists
    paint();
  }

  async function refresh({ keepDrafts = false } = {}) {
    if (state.disposed) return;
    try {
      const answer = await state.callRpc("issues.get", { issue_id: state.issueId });
      if (state.disposed) return;
      take(answer, { keepDrafts });
      paint();
      await writeIssueRecord(state.deviceId, state.projectId, state.issueId, issueRecord(answer.issue, answer.timeline));
    } catch (error) {
      if (state.disposed) return;
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
      await refresh({ keepDrafts: true });
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

  async function sendComment() {
    const body = state.draft.trim();
    if (!body || state.sending) return;
    state.sending = true;
    paint();
    try {
      await state.callRpc("issues.comment", { issue_id: state.issueId, body });
      state.draft = "";
      await refresh({ keepDrafts: true });
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

  function wireComposer() {
    const form = host.querySelector("[data-issue-composer]");
    const field = host.querySelector("#issue-comment");
    field.oninput = () => {
      const wasEmpty = !state.draft.trim();
      state.draft = field.value;
      // Repaint only when the send button's own state moves: a repaint per
      // keystroke would take the caret with it.
      if (wasEmpty !== !state.draft.trim()) {
        paint();
        host.querySelector("#issue-comment")?.focus();
      }
    };
    form.onsubmit = (event) => {
      event.preventDefault();
      void sendComment();
    };
  }

  function wire() {
    wireRail();
    wireComposer();
  }

  // ---- lifecycle -----------------------------------------------------------

  paint();
  void paintFromCache().then(() => refresh());
  // No cadence: nothing in this client polls. An item that names this issue
  // is what re-reads it, and the pass behind that (core/cacheSync.js) is the
  // whole of the safety net.
  const watcher = watchChanges({
    refresh: () => void refresh({ keepDrafts: true }),
    entity: state.projectId,
    deviceId: state.deviceId,
    kinds: ["issues"],
    mode: "realtime",
    onChanges: (items) => {
      if (namesIssue(items, state.issueId)) void refresh({ keepDrafts: true });
    },
  });

  return {
    /** The feed moved: the workspaces a link points at and the agents an
     *  assignee is named by may have. Nothing is re-read from the bridge. */
    feedMoved: paint,
    dispose() {
      state.disposed = true;
      watcher.dispose();
      state.picker?.close?.();
    },
  };
}
