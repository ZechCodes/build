// The one create surface: a modal that files an issue or starts work on a
// branch in a project, with a tab for each. The toolbar's work menu and the
// rail's project blocks both open it, so creating reads the same wherever it
// starts.
//
// The Branch tab is a picker over every branch the project has. The text field
// does both jobs at once: it filters the list, and it names the branch to cut
// when it matches none. What a row does depends on where its branch already is
// — branchPickerModel decides that, and this file only presses what it decides.
// An issue asks for what you want, and carries the harness question — an
// issue's agent is chosen from the moment it is filed, while a new branch
// carries no agent at all until something starts one there. Switching tabs
// keeps what was typed on each; a refused create repaints with the words and
// the caret where they were.
//
// Nothing is dispatched by either create: the first message sent there is what
// starts an agent.

import { esc } from "./text.js";
import { App, go } from "../app.js";
import { refreshFeed } from "./taskFeed.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import { agentChoiceParams, agentChoicePanelHtml, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import { branchNamePreview } from "./toolbarModel.js";
import { branchPickerRows, branchStartRoute, cutNewRow } from "./branchPickerModel.js";
import { modalDialogHtml, openModal } from "./modal.js";

/** The two things a project can hold, in the order the tabs offer them. */
export const CREATE_KINDS = ["branch", "issue"];

/** Names the form's three harness controls, so its panel and compose's can be
 *  open at once without either answering for the other. */
const CHOICE_PREFIX = "create-choice";

/** What each tab asks for. One field, because one field is all it needs. */
const CREATE_COPY = {
  branch: {
    tab: "Branch",
    hint: "Start work on any branch of the project, or name a new one. Nothing is dispatched — the first message you send starts an agent there.",
    placeholder: "Find a branch, or name a new one…",
    label: "Branch",
    empty: "Name it first.",
    failed: "Couldn't create the branch",
  },
  issue: {
    tab: "Issue",
    hint: "Say what you want. No planning agent starts until you send the first message.",
    placeholder: "e.g. Add a /health endpoint that returns build SHA and uptime…",
    label: "Goal",
    empty: "Describe the issue first.",
    failed: "Couldn't file the issue",
  },
};

const catalog = () => App.modelCatalog || { providers: [] };

/** The rows the Branch tab is showing for what has been typed. Read off state
 *  wherever they are needed, so the painted list and the pressed row can never
 *  be two different answers. */
const pickerRows = (state) =>
  branchPickerRows({ projectId: state.projectId, branches: state.branches, query: state.values.branch || "" });

function branchRowHtml(row, index, highlight) {
  const marks = [
    row.remote ? `<span class="branch-row-mark">${esc(row.remote)} only</span>` : "",
    row.detail ? `<span class="branch-row-detail">${esc(row.detail)}</span>` : "",
  ].join("");
  return `<button class="branch-row${index === highlight ? " on" : ""}" type="button" role="option" aria-selected="${
    index === highlight ? "true" : "false"
  }" data-branch-pick="${index}">
      <span class="branch-row-verb">${esc(row.verb)}</span>
      <span class="branch-row-name">${esc(row.name)}</span>
      ${marks}
    </button>`;
}

/** The branch list under the field: the rows, or the one line saying why there
 *  are none. A listing that could not be read never stops a branch being cut
 *  by name, so it is a note beside the field and not the form's error. */
function branchPickerHtml(state) {
  const rows = pickerRows(state);
  const note = state.branchesError
    ? `<div class="sub branch-picker-note">Couldn't list branches: ${esc(state.branchesError)}</div>`
    : rows.length || !state.branchesLoaded
      ? ""
      : `<div class="sub branch-picker-note">No branch matches.</div>`;
  return `<div class="branch-picker" role="listbox" aria-label="Branches">${rows
    .map((row, index) => branchRowHtml(row, index, state.highlight))
    .join("")}${note}</div>`;
}

/** The dialog's inside. `state`: { projectId, projectName, kind, values, busy,
 *  error, choice, choiceOpen, branches, branchesError, branchesLoaded,
 *  highlight }. Everything user-supplied is escaped. */
export function createWorkBodyHtml(state) {
  const copy = CREATE_COPY[state.kind];
  const value = state.values[state.kind] || "";
  const tabs = CREATE_KINDS.map(
    (kind) =>
      `<button class="btn seg${kind === state.kind ? " primary" : ""}" type="button" role="tab" aria-selected="${
        kind === state.kind ? "true" : "false"
      }" data-create-tab="${kind}">${CREATE_COPY[kind].tab}</button>`,
  ).join("");
  const field =
    state.kind === "issue"
      ? `<textarea id="create-work-input" rows="3" placeholder="${esc(copy.placeholder)}">${esc(value)}</textarea>`
      : `<input id="create-work-input" type="text" class="path" placeholder="${esc(copy.placeholder)}" autocomplete="off" value="${esc(value)}" />`;
  const under =
    state.kind === "issue"
      ? agentChoicePanelHtml(catalog(), state.choice, { prefix: CHOICE_PREFIX, open: state.choiceOpen })
      : branchPickerHtml(state);
  return `<h3>New in ${esc(state.projectName)}</h3>
    <div class="segmented create-tabs" role="tablist">${tabs}</div>
    <div class="sub create-hint">${esc(copy.hint)}</div>
    <label class="create-label" for="create-work-input">${esc(copy.label)}</label>
    ${field}
    ${under}
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <span class="dim mono create-preview" id="create-work-preview">${state.kind === "branch" ? esc(branchNamePreview(value)) : ""}</span>
      <button class="btn" data-create-cancel type="button">Cancel</button>
      <button class="btn primary" data-create-go type="button"${state.busy ? " disabled" : ""}>${state.busy ? "creating…" : "Create"}</button>
    </div>`;
}

export function createWorkHtml(state) {
  return modalDialogHtml(createWorkBodyHtml(state), { className: "modal-create" });
}

/** Inert by contract: the record exists, and nothing runs behind it until the
 *  first message (the bridge dispatches planning on that post). An empty
 *  harness choice sends nothing, and the daemon's own default stands. */
async function createIssue(projectId, goal, choice) {
  const created = await App.call("issue.create", {
    goal,
    project_id: projectId,
    dispatch: false,
    ...agentChoiceParams(catalog(), choice),
  });
  return {
    route: { name: "issue", projectId: created.project_id || projectId, id: created.issue_id || created.plan_id },
    focusComposer: false,
  };
}

/** Press a picker row: make the one call it names — a checkout, an adoption, or
 *  nothing at all for a branch a run already owns — and say where it lands. */
async function startBranch(projectId, row) {
  const answer = row.call ? await App.call(row.call.method, row.call.params) : null;
  return { route: branchStartRoute(projectId, row, answer), focusComposer: row.focusComposer };
}

/**
 * Open the create modal on `kind`'s tab, for the project named. `navigate` is
 * how the thing made is opened — the rail passes its own, which puts the rail
 * away on a narrow viewport. Returns { close }.
 */
export function openCreateWork({ projectId, projectName, kind = "branch", navigate = go }) {
  const state = {
    projectId,
    projectName: projectName || projectId,
    kind: CREATE_KINDS.includes(kind) ? kind : "branch",
    values: { branch: "", issue: "" },
    busy: false,
    error: "",
    choice: loadAgentDefaults(),
    choiceOpen: false,
    branches: [],
    branchesError: "",
    branchesLoaded: false,
    // Nothing is highlighted until there is text to highlight for, so Enter on
    // an untouched field still asks to be told a name rather than opening
    // whichever branch happens to sit at the top of the list.
    highlight: -1,
  };
  const { body, close } = openModal({ dialogHtml: createWorkHtml(state), scrimId: "create-scrim" });

  function paint() {
    // The typed answer is state, not something the DOM happens to be holding:
    // a refused create repaints this form, and it must repaint with what was
    // typed — caret included.
    const typing = body.querySelector("#create-work-input");
    const caret = typing && document.activeElement === typing ? typing.selectionStart : (state.values[state.kind] || "").length;
    body.innerHTML = createWorkBodyHtml(state);
    wire(caret);
  }

  function wire(caret) {
    body.querySelectorAll("[data-create-tab]").forEach((tab) => {
      tab.onclick = () => {
        state.kind = tab.dataset.createTab;
        state.error = "";
        paint();
        loadBranches();
      };
    });
    const input = body.querySelector("#create-work-input");
    input.focus();
    input.setSelectionRange(caret, caret);
    input.oninput = () => {
      state.values[state.kind] = input.value;
      if (state.kind !== "branch") return;
      // The branch the text would cut leads the list, so highlighting the top
      // row keeps Enter on what this field has always done.
      state.highlight = input.value.trim() ? 0 : -1;
      paint();
    };
    input.onkeydown = (event) => {
      if (state.kind === "branch" && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
        event.preventDefault();
        moveHighlight(event.key === "ArrowDown" ? 1 : -1);
        return;
      }
      if (event.key === "Enter" && (state.kind === "branch" || event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        submit();
      }
    };
    body.querySelectorAll("[data-branch-pick]").forEach((row) => {
      row.onclick = () => {
        state.highlight = Number(row.dataset.branchPick);
        submit();
      };
    });
    body.querySelector("[data-create-cancel]").onclick = close;
    body.querySelector("[data-create-go]").onclick = submit;
    wireChoice();
  }

  function moveHighlight(step) {
    const last = pickerRows(state).length - 1;
    state.highlight = Math.min(last, Math.max(-1, state.highlight + step));
    paint();
  }

  function wireChoice() {
    const holder = body.querySelector(".agent-choice");
    if (!holder) return;
    holder.querySelector("[data-agent-choice-toggle]").onclick = () => {
      state.choiceOpen = !state.choiceOpen;
      paint();
    };
    const onChange = (changed) => () => {
      state.choice = reconcileAgentChoice(readAgentChoice(body, CHOICE_PREFIX), changed);
      paint();
    };
    const control = (field) => holder.querySelector(`#${CHOICE_PREFIX}-${field}`);
    control("provider").onchange = onChange({ providerChanged: true });
    control("model").onchange = onChange({ modelChanged: true });
    control("effort").onchange = onChange({});
  }

  /** The project's branches, asked for once, when the tab that shows them is
   *  the one being looked at. */
  async function loadBranches() {
    if (state.kind !== "branch" || state.branchesLoaded || !projectId) return;
    state.branchesLoaded = true;
    try {
      const listing = await App.call("git.branches", { project_id: projectId });
      state.branches = (listing && listing.branches) || [];
    } catch (error) {
      state.branchesError = error.message || String(error);
    }
    paint();
  }

  /** What the human is asking for: the row they highlighted, or — with none —
   *  the branch the typed text would cut, which is what this field meant
   *  before it had a list under it. */
  function pickedRow(value) {
    const rows = pickerRows(state);
    return rows[state.highlight] || (value ? cutNewRow(projectId, value) : null);
  }

  async function submit() {
    if (state.busy) return;
    const { kind } = state;
    const value = (state.values[kind] || "").trim();
    const row = kind === "branch" ? pickedRow(value) : null;
    if (kind === "branch" ? !row : !value) {
      state.error = CREATE_COPY[kind].empty;
      paint();
      return;
    }
    if (!projectId) {
      state.error = "No project to create in.";
      paint();
      return;
    }
    state.busy = true;
    state.error = "";
    paint();
    try {
      const started = kind === "branch" ? await startBranch(projectId, row) : await createIssue(projectId, value, state.choice);
      close();
      refreshFeed();
      // A checkout with nobody in it opens on the ghost composer, and that is
      // exactly where typing the first message belongs.
      App.focusComposerOnMount = started.focusComposer;
      navigate(started.route);
    } catch (error) {
      state.busy = false;
      state.error = error.message || String(error);
      paint();
    }
  }

  wire((state.values[state.kind] || "").length);
  loadBranches();
  return { close };
}
