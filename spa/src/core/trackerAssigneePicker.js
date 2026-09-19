// The assignee picker, as a modal.
//
// Opened from a list row, from a kanban card and from the issue page's rail —
// one control, because "hand this to somebody" is one act wherever it is
// reached from. The control itself is core/trackerAssigneeControl.js, shared
// with the new-issue form; this is the dialog around it: the note, the press,
// and what the press says while it is running.
//
// The press is deliberately not called "Save". Assigning is dispatching: for
// three of the five kinds nothing starts, and for the other two a workspace is
// cut or an agent is started. The button says which.

import { esc, messageOf } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import {
  assigneeControlHtml,
  draftAssignParams,
  draftStartsWork,
  emptyAssigneeDraft,
  readAssigneeDraft,
  wireAssigneeControl,
} from "./trackerAssigneeControl.js";

const PREFIX = "issue-assign";

const pressLabel = (state) => {
  if (state.busy) return "assigning…";
  return draftStartsWork(state.options, state.draft) ? "Assign and start" : "Assign";
};

const titleOf = (issue) => `Assign #${issue.number ?? ""}`;

export function assigneePickerBodyHtml(state) {
  return `<h3>${esc(titleOf(state.issue))}</h3>
    <p class="sub create-hint">${esc(state.issue.title || "")}</p>
    ${assigneeControlHtml(state.options, state.draft, { prefix: PREFIX, catalog: state.catalog })}
    <label class="create-label" for="${PREFIX}-note">Note <span class="sub">(delivered with the issue, not stored on it)</span></label>
    <textarea id="${PREFIX}-note" rows="2" placeholder="Anything the issue itself does not say">${esc(state.note)}</textarea>
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <button class="btn" data-assign-cancel type="button"${state.busy ? " disabled" : ""}>Cancel</button>
      <button class="btn primary" data-assign-go type="button"${state.busy ? " disabled" : ""}>${esc(pressLabel(state))}</button>
    </div>`;
}

/**
 * Open the picker on one issue.
 *
 * `options` is what core/trackerAssignee.js offered for this project, `current`
 * the option id its assignee already is, and `catalog` the creation device's
 * `models.list`. `onAssigned` is handed the whole `issues.assign` answer — the
 * issue AND the dispatch — because what was started is the half the caller most
 * needs to say something about.
 */
export function openAssigneePicker({ issue, options, current = "none", catalog = null, callRpc, onAssigned = null }) {
  const state = {
    issue,
    options,
    catalog,
    draft: emptyAssigneeDraft(current),
    note: "",
    busy: false,
    error: "",
  };
  let dismissed = false;
  const modal = openModal({
    dialogHtml: modalDialogHtml(assigneePickerBodyHtml(state), { className: "modal-create" }),
    scrimId: "issue-assign-scrim",
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
        issueId: issue.id,
        note: state.note,
      });
      const answer = await callRpc("issues.assign", params);
      if (dismissed) return;
      await close();
      onAssigned?.(answer);
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
