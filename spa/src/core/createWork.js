// Workspace creation is the only active create flow. A workspace materializes
// every source registered to its project and inherits the project's isolation
// unless the user explicitly chooses an override.

import { App, go } from "../app.js";
import { isolationOptionsHtml } from "./isolation.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { workspaceRoute } from "./projectModel.js";
import { replyOrNothing } from "./session.js";
import { esc } from "./text.js";

export const CREATE_KINDS = ["workspace"];

export function workspaceCreateParams({ projectId, name = "", isolation = "" }) {
  return {
    project_id: projectId,
    ...(name.trim() ? { name: name.trim() } : null),
    ...(isolation ? { isolation } : null),
  };
}

const canCreate = (state) => Boolean(state.projectId && App.call);
const creationEnded = (dismissed, answer) => dismissed || !answer;

export function createWorkBodyHtml(state) {
  return `<h3>New workspace in ${esc(state.projectName)}</h3>
    <p class="sub create-hint">Copies every source in this project into one durable workspace.</p>
    <label class="create-label" for="create-work-input">Name <span class="dim">optional</span></label>
    <input id="create-work-input" type="text" placeholder="Workspace" autocomplete="off" value="${esc(state.name)}" />
    <label class="create-label" for="create-work-isolation">Isolation</label>
    <select id="create-work-isolation">
      ${isolationOptionsHtml(state.isolation, null, { inheritLabel: "Inherit project setting" })}
    </select>
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <button class="btn" data-create-cancel type="button">Cancel</button>
      <button class="btn primary" data-create-go type="button"${state.busy ? " disabled" : ""}>${state.busy ? "creating…" : "Create workspace"}</button>
    </div>`;
}

export function createWorkHtml(state) {
  return modalDialogHtml(createWorkBodyHtml(state), { className: "modal-create" });
}

export function openCreateWork({ projectId, projectName, navigate = go }) {
  const state = { projectId, projectName: projectName || projectId || "project", name: "", isolation: "", busy: false, error: "" };
  let dismissed = false;
  let close;
  const dismiss = () => {
    dismissed = true;
    return close();
  };
  const modal = openModal({ dialogHtml: createWorkHtml(state), scrimId: "create-scrim", onClose: () => { dismissed = true; } });
  close = modal.close;
  const { body } = modal;

  const paint = () => {
    const input = body.querySelector("#create-work-input");
    const caret = input?.selectionStart ?? state.name.length;
    body.innerHTML = createWorkBodyHtml(state);
    wire(caret);
  };

  const submit = async () => {
    if (state.busy) return;
    if (!canCreate(state)) {
      state.error = "No connected project to create in.";
      paint();
      return;
    }
    state.busy = true;
    state.error = "";
    paint();
    try {
      const answer = await replyOrNothing(App.call("workspace.create", workspaceCreateParams(state)));
      if (creationEnded(dismissed, answer)) return;
      const workspace = answer.workspace || answer;
      const route = workspaceRoute(workspace);
      if (!route) throw new Error("The workspace was created but returned no usable identity.");
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
  return { close: dismiss };
}
