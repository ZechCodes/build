// Bridges without environments retain the original current-project dialog.
// The capability gate lives at the entry; this module has no picker or drafts.
import { go } from "../app.js";
import { createLegacyWorkspace } from "./createWorkspaceTargets.js";
import { isolationOptionsHtml } from "./isolation.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { esc } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";

export function createWorkspaceLegacyBodyHtml(state) {
  return `<h3>New workspace in ${esc(state.projectName)}</h3>
    <p class="sub create-hint">Creates a separate checkout of the project sources.</p>
    <label class="create-label" for="create-work-input">Workspace label</label>
    <input id="create-work-input" type="text" placeholder="Workspace" ${fieldTraits("identifier", "go")} value="${esc(state.name)}"${state.busy ? " disabled" : ""} />
    <label class="create-label" for="create-work-isolation">Isolation</label>
    <select id="create-work-isolation"${state.busy ? " disabled" : ""}>
      ${isolationOptionsHtml(state.isolation, null, { inheritLabel: "Inherit project setting" })}
    </select>
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <button class="btn" data-create-cancel type="button"${state.busy ? " disabled" : ""}>Cancel</button>
      <button class="btn primary" data-create-go type="button"${state.busy ? " disabled" : ""}>${state.busy ? "creating…" : "Create workspace"}</button>
    </div>`;
}

export function createWorkspaceLegacyHtml(state) {
  return modalDialogHtml(createWorkspaceLegacyBodyHtml(state), { className: "modal-create" });
}

export function openCreateWorkspaceLegacy({ projectId, deviceId, projectName, name = "", isolation = "", navigate = go, onClose = null }) {
  const state = { projectId, deviceId, projectName: projectName || projectId || "project", name, isolation, busy: false, error: "" };
  let dismissed = false;
  let close;
  const dismiss = () => {
    if (state.busy) return;
    dismissed = true;
    return close();
  };
  const modal = openModal({ dialogHtml: createWorkspaceLegacyHtml(state), scrimId: "create-scrim", canDismiss: () => !state.busy, onClose: () => { dismissed = true; onClose?.(); } });
  close = modal.close;
  const { body } = modal;

  const paint = () => {
    const input = body.querySelector("#create-work-input");
    const caret = input?.selectionStart ?? state.name.length;
    body.innerHTML = createWorkspaceLegacyBodyHtml(state);
    wire(caret);
  };

  const submit = async () => {
    if (state.busy) return;
    if (!state.projectId) {
      state.error = "No connected project to create in.";
      paint();
      return;
    }
    state.busy = true;
    state.error = "";
    paint();
    try {
      const route = await createLegacyWorkspace({
        target: { kind: "project", id: state.projectId, deviceId }, name: state.name, isolation: state.isolation,
      });
      if (dismissed) return;
      await close();
      navigate(route);
    } catch (error) {
      if (dismissed) return;
      state.busy = false;
      state.error = error.message || String(error);
      paint();
    }
  };

  function wire(caret = state.name.length) {
    const input = body.querySelector("#create-work-input");
    input.focus();
    input.setSelectionRange(caret, caret);
    input.oninput = () => { state.name = input.value; };
    input.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      submit();
    };
    body.querySelector("#create-work-isolation").onchange = (event) => { state.isolation = event.target.value; };
    body.querySelector("[data-create-cancel]").onclick = dismiss;
    body.querySelector("[data-create-go]").onclick = submit;
  }

  wire();
  return { close: dismiss, detach: dismiss, ready: Promise.resolve() };
}
