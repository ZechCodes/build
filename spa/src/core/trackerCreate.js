// Filing an issue: a small form, because an issue is cheap and a form that
// asks for much is a form nobody fills in.
//
// Title and body are the issue. Labels and priority are the two facts that are
// far easier to say now than to come back for. The assignee is here because
// assigning is dispatching — filing an issue AND handing it to an agent in one
// press is the whole of what makes this tracker Build's rather than a copy of
// GitHub's, and `issues.create` runs the whole of `issues.assign` inside its
// own transaction when it is given one.
//
// The answer carries the dispatch beside the issue, so the caller can say what
// was started rather than only that something was filed.

import { esc, messageOf } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { PRIORITIES } from "./trackerModel.js";
import {
  assigneeControlHtml,
  draftAssignee,
  draftStartsWork,
  emptyAssigneeDraft,
  wireAssigneeControl,
} from "./trackerAssigneeControl.js";

const PREFIX = "issue-new";

/** Labels as the user types them — commas, because that is how anyone writes a
 *  short list. Trimmed, emptied out and deduped, which is what the verb does to
 *  them anyway; doing it here means the form shows what will be stored. */
export const labelsFromText = (text) => [
  ...new Set(String(text || "").split(",").map((label) => label.trim()).filter(Boolean)),
];

const priorityOptionsHtml = (chosen) =>
  PRIORITIES.map(
    (priority) => `<option value="${esc(priority.id)}"${priority.id === chosen ? " selected" : ""}>${esc(priority.label)}</option>`,
  ).join("");

const pressLabel = (state) => {
  if (state.busy) return "filing…";
  return draftStartsWork(state.options, state.draft) ? "File and start" : "File issue";
};

export function createIssueBodyHtml(state) {
  return `<h3>New issue in ${esc(state.projectName)}</h3>
    <label class="create-label" for="${PREFIX}-title">Title</label>
    <input id="${PREFIX}-title" type="text" autocomplete="off" placeholder="What should be done" value="${esc(state.title)}"${state.busy ? " disabled" : ""} />
    <label class="create-label" for="${PREFIX}-body">Description <span class="sub">(markdown)</span></label>
    <textarea id="${PREFIX}-body" rows="5" placeholder="Anything the title leaves out"${state.busy ? " disabled" : ""}>${esc(state.body)}</textarea>
    <label class="create-label" for="${PREFIX}-labels">Labels <span class="sub">(comma separated)</span></label>
    <input id="${PREFIX}-labels" type="text" autocomplete="off" placeholder="bug, ui" value="${esc(state.labels)}"${state.busy ? " disabled" : ""} />
    <label class="create-label" for="${PREFIX}-priority">Priority</label>
    <select id="${PREFIX}-priority"${state.busy ? " disabled" : ""}>${priorityOptionsHtml(state.priority)}</select>
    ${assigneeControlHtml(state.options, state.draft, { prefix: PREFIX, catalog: state.catalog })}
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <button class="btn" data-create-cancel type="button"${state.busy ? " disabled" : ""}>Cancel</button>
      <button class="btn primary" data-create-go type="button"${state.busy ? " disabled" : ""}>${esc(pressLabel(state))}</button>
    </div>`;
}

/** The form as `issues.create` params. A field nobody filled in is left off:
 *  the verb's own defaults are the record's defaults, and sending an empty
 *  string instead would store one. */
export function createIssueParams(state, assignee) {
  const labels = labelsFromText(state.labels);
  return {
    project_id: state.projectId,
    title: state.title.trim(),
    ...(state.body.trim() ? { body: state.body } : null),
    ...(labels.length ? { labels } : null),
    ...(state.priority && state.priority !== "none" ? { priority: state.priority } : null),
    ...(assignee ? { assignee } : null),
  };
}

/**
 * Open the form.
 *
 * `onFiled` is handed the whole answer — `{issue, dispatch}` — because the
 * caller's next act depends on which it was: an issue that dispatched has a
 * conversation to offer, and one that did not has only itself.
 */
export function openCreateIssue({ projectId, projectName, options, catalog = null, callRpc, onFiled = null }) {
  const state = {
    projectId,
    projectName: projectName || projectId || "this project",
    options,
    catalog,
    title: "",
    body: "",
    labels: "",
    priority: "none",
    draft: emptyAssigneeDraft("none"),
    busy: false,
    error: "",
  };
  let dismissed = false;
  const modal = openModal({
    dialogHtml: modalDialogHtml(createIssueBodyHtml(state), { className: "modal-create modal-issue-new" }),
    scrimId: "issue-new-scrim",
    canDismiss: () => !state.busy,
    onClose: () => {
      dismissed = true;
    },
  });
  const { body, close } = modal;

  const paint = () => {
    body.innerHTML = createIssueBodyHtml(state);
    wire();
  };

  const readFields = () => {
    state.title = body.querySelector(`#${PREFIX}-title`).value;
    state.body = body.querySelector(`#${PREFIX}-body`).value;
    state.labels = body.querySelector(`#${PREFIX}-labels`).value;
    state.priority = body.querySelector(`#${PREFIX}-priority`).value;
  };

  const submit = async () => {
    if (state.busy) return;
    readFields();
    if (!state.title.trim()) {
      state.error = "An issue needs a title.";
      paint();
      return;
    }
    state.busy = true;
    state.error = "";
    paint();
    try {
      const assignee = draftAssignee(state.options, state.draft, state.catalog);
      const answer = await callRpc("issues.create", createIssueParams(state, assignee));
      if (dismissed) return;
      await close();
      onFiled?.(answer);
    } catch (error) {
      if (dismissed) return;
      state.busy = false;
      state.error = messageOf(error);
      paint();
    }
  };

  function wire() {
    wireAssigneeControl(body, state.draft, {
      prefix: PREFIX,
      onDraft: (draft) => {
        readFields();
        state.draft = draft;
        paint();
      },
    });
    body.querySelector("[data-create-cancel]").onclick = () => {
      if (!state.busy) close();
    };
    body.querySelector("[data-create-go]").onclick = submit;
    const title = body.querySelector(`#${PREFIX}-title`);
    title.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      submit();
    };
  }

  wire();
  body.querySelector(`#${PREFIX}-title`).focus();
  return { close };
}
