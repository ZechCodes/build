// The assignee picker, as a modal.
//
// Opened from a list row, from a kanban card and from the task page's rail —
// one control, because "hand this to somebody" is one act wherever it is
// reached from. The control itself is core/trackerAssigneeControl.js, shared
// with the new-task form; this is the dialog around it: the note, the press,
// and what the press says while it is running.
//
// The press is deliberately not called "Save". Assigning is dispatching: for
// three of the five kinds nothing starts, and for the other two a workspace is
// cut or an agent is started. The button says which.

import { esc, messageOf } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { assignRefusalText } from "./trackerAssignee.js";
import {
  assigneeControlHtml,
  draftAssignParams,
  draftStartsWork,
  draftWaitsOnAWorkspace,
  emptyAssigneeDraft,
  readAssigneeDraft,
  wireAssigneeControl,
} from "./trackerAssigneeControl.js";
import { fieldTraits } from "./fieldTraits.js";

const PREFIX = "task-assign";

/**
 * What the press says.
 *
 * Idle, it says what it is about to do. Working, it says what is actually
 * being waited on: cutting a checkout is the slowest thing in the tracker —
 * seconds on a small repository, minutes on a large one — and a button that
 * says "assigning…" for two minutes reads as a hung dialog rather than as
 * work happening. Every other kind answers in milliseconds and nobody reads
 * its label at all.
 */
const pressLabel = (state) => {
  if (state.busy) return waitingOnAWorkspace(state) ? "cutting the workspace…" : "assigning…";
  return draftStartsWork(state.options, state.draft) ? "Assign and start" : "Assign";
};

const waitingOnAWorkspace = (state) => draftWaitsOnAWorkspace(state.options, state.draft);

/** The line under a press that is going to be held for a while. It says what
 *  is happening and that leaving is safe, because the two questions a reader
 *  asks of a dialog that has not moved are "is it stuck" and "can I go". */
const waitingHtml = (state) =>
  state.busy && waitingOnAWorkspace(state)
    ? `<p class="sub task-assign-waiting" role="status">Cutting the checkout and starting the agent. This can take a minute on a large repository.</p>`
    : "";

const titleOf = (task) => `Assign #${task.number ?? ""}`;

export function assigneePickerBodyHtml(state) {
  return `<h3>${esc(titleOf(state.task))}</h3>
    <p class="sub create-hint">${esc(state.task.title || "")}</p>
    ${assigneeControlHtml(state.options, state.draft, { prefix: PREFIX, catalog: state.catalog })}
    <label class="create-label" for="${PREFIX}-note">Note <span class="sub">(delivered with the task, not stored on it)</span></label>
    <textarea id="${PREFIX}-note" rows="2" ${fieldTraits("prose")} placeholder="Anything the task itself does not say">${esc(state.note)}</textarea>
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    ${waitingHtml(state)}
    <div class="row create-row">
      <button class="btn" data-assign-cancel type="button"${state.busy ? " disabled" : ""}>Cancel</button>
      <button class="btn primary" data-assign-go type="button"${state.busy ? " disabled" : ""}>${esc(pressLabel(state))}</button>
    </div>`;
}

/**
 * Open the picker on one task.
 *
 * `options` is what core/trackerAssignee.js offered for this project, `current`
 * the option id its assignee already is, and `catalog` the creation device's
 * `models.list`. `onAssigned` is handed the whole `tasks.assign` answer — the
 * task AND the dispatch — because what was started is the half the caller most
 * needs to say something about.
 */
export function openAssigneePicker({ task, options, current = "none", catalog = null, callRpc, onAssigned = null, note = "" }) {
  const state = {
    task,
    options,
    catalog,
    draft: emptyAssigneeDraft(current),
    note,
    busy: false,
    error: "",
  };
  let dismissed = false;
  const modal = openModal({
    dialogHtml: modalDialogHtml(assigneePickerBodyHtml(state), { className: "modal-create" }),
    scrimId: "task-assign-scrim",
    canDismiss: () => !state.busy,
    onClose: () => {
      dismissed = true;
    },
  });
  const { body, close } = modal;

  const paint = () => {
    body.innerHTML = assigneePickerBodyHtml(state);
    wire();
  };

  const submit = async () => {
    if (state.busy) return;
    state.busy = true;
    state.error = "";
    paint();
    try {
      const params = draftAssignParams(state.options, state.draft, state.catalog, {
        taskId: task.id,
        note: state.note,
      });
      const answer = await callRpc("tasks.assign", params);
      if (dismissed) return;
      await close();
      onAssigned?.(answer);
    } catch (error) {
      if (dismissed) return;
      state.busy = false;
      // Nothing was written: a failed cut leaves the task unassigned, in its
      // old column, with no events. There is nothing to reconcile — the dialog
      // stays up with the draft intact, so pressing again IS the retry.
      state.error = assignRefusalText(messageOf(error));
      paint();
    }
  };

  function wire() {
    wireAssigneeControl(body, state.draft, {
      prefix: PREFIX,
      onDraft: (draft) => {
        state.draft = draft;
        paint();
      },
    });
    const note = body.querySelector(`#${PREFIX}-note`);
    if (note) note.oninput = () => { state.note = note.value; };
    body.querySelector("[data-assign-cancel]").onclick = () => {
      if (!state.busy) close();
    };
    body.querySelector("[data-assign-go]").onclick = submit;
  }

  wire();
  return {
    close,
    /** The catalog arrives after the dialog does on a cold device; the panel is
     *  repainted when it lands so an agent choice is offered rather than the
     *  harness's default silently standing. What was typed is kept. */
    setCatalog(catalog) {
      if (dismissed || state.catalog === catalog) return;
      state.catalog = catalog;
      state.draft = readAssigneeDraft(body, state.draft, PREFIX);
      paint();
    },
  };
}
