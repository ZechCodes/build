// Workspace creation is the only active create flow. A workspace materializes
// every source registered to its project and inherits the project's isolation
// unless the user explicitly chooses an override.
//
// A project belongs to one machine, so the dialog is opened with the device the
// project is on: that machine is asked for the workspace, and the route the
// created workspace opens at names it.

import { go } from "../app.js";
import { canAnswer, contextFor } from "./deviceContexts.js";
import { deviceCall } from "./inboxDevices.js";
import { isolationOptionsHtml } from "./isolation.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { workspaceRoute } from "./projectModel.js";
import { esc } from "./text.js";

export const CREATE_KINDS = ["workspace"];

export function workspaceCreateParams({ projectId, name = "", isolation = "" }) {
  return {
    project_id: projectId,
    name,
    ...(isolation ? { isolation } : null),
  };
}

const canCreate = (state) => Boolean(state.projectId && canAnswer(contextFor(state.deviceId)));

function createdWorkspaceRoute(answer) {
  const workspace = answer?.workspace || answer;
  if (workspace?.status === "failed") {
    throw new Error(workspace.directories?.find((directory) => directory.error)?.error || "Workspace creation failed.");
  }
  const route = workspace && workspaceRoute(workspace);
  if (!route) throw new Error("The workspace was created but returned no usable identity.");
  return route;
}

export function createWorkBodyHtml(state) {
  return `<h3>New workspace in ${esc(state.projectName)}</h3>
    <p class="sub create-hint">Copies every source in this project into one durable workspace.</p>
    <label class="create-label" for="create-work-input">Name</label>
    <input id="create-work-input" type="text" placeholder="Workspace" autocomplete="off" value="${esc(state.name)}"${state.busy ? " disabled" : ""} />
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

export function createWorkHtml(state) {
  return modalDialogHtml(createWorkBodyHtml(state), { className: "modal-create" });
}

export function openCreateWork({ projectId, deviceId, projectName, navigate = go }) {
  const state = { projectId, deviceId, projectName: projectName || projectId || "project", name: "", isolation: "", busy: false, error: "" };
  // Read at the press, never captured at the mount: a machine that was away
  // when the dialog opened is asked the moment it is back, which is the way the
  // composer and the capture page ask too (core/inboxDevices.js).
  const askDevice = (method, params) => deviceCall(deviceId)(method, params);
  let dismissed = false;
  let close;
  const dismiss = () => {
    if (state.busy) return;
    dismissed = true;
    return close();
  };
  const modal = openModal({ dialogHtml: createWorkHtml(state), scrimId: "create-scrim", canDismiss: () => !state.busy, onClose: () => { dismissed = true; } });
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
      const answer = await askDevice("workspace.create", workspaceCreateParams(state));
      if (dismissed) return;
      const route = createdWorkspaceRoute(answer);
      await close();
      // The machine that made it is the machine it is on: the answer to a fresh
      // workspace.create is not a feed row and carries no device of its own.
      navigate({ ...route, deviceId });
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
