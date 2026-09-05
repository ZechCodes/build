// The one create surface: a modal that files an issue or cuts a branch in a
// project, with a tab for each. The toolbar's work menu and the rail's project
// blocks both open it, so creating reads the same wherever it starts.
//
// A branch asks for one thing, a name, and shows the branch the daemon will cut
// for it. An issue asks for what you want, and carries the harness question —
// an issue's agent is chosen from the moment it is filed, while a new branch
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
import { modalDialogHtml, openModal } from "./modal.js";
import { replyOrNothing } from "./session.js";

/** The two things a project can hold, in the order the tabs offer them. */
export const CREATE_KINDS = ["branch", "issue"];

/** Names the form's three harness controls, so its panel and compose's can be
 *  open at once without either answering for the other. */
const CHOICE_PREFIX = "create-choice";

/** What each tab asks for. One field, because one field is all it needs. */
const CREATE_COPY = {
  branch: {
    tab: "Branch",
    hint: "A checkout and a branch of its own. Nothing is dispatched — the first message you send starts an agent there.",
    placeholder: "e.g. mascot model spike",
    label: "Name",
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

/** The dialog's inside. `state`: { projectName, kind, values, busy, error,
 *  choice, choiceOpen }. Everything user-supplied is escaped. */
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
  const choice =
    state.kind === "issue" ? agentChoicePanelHtml(catalog(), state.choice, { prefix: CHOICE_PREFIX, open: state.choiceOpen }) : "";
  return `<h3>New in ${esc(state.projectName)}</h3>
    <div class="segmented create-tabs" role="tablist">${tabs}</div>
    <div class="sub create-hint">${esc(copy.hint)}</div>
    <label class="create-label" for="create-work-input">${esc(copy.label)}</label>
    ${field}
    ${choice}
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

/** Cut the branch, and say where it opens — or nothing, when the reply does not
 *  name it. Either way the board is already carrying the row (the daemon puts
 *  it there before the git runs) and the record settles into it. */
async function createBranch(projectId, name) {
  const created = await replyOrNothing(App.call("worktree.create", { project_id: projectId, name }));
  if (!created || !created.branch) return null;
  return { name: "branch", projectId: created.project_id || projectId, branch: created.branch, tab: "changes" };
}

/** Inert by contract: the record exists, and nothing runs behind it until the
 *  first message (the bridge dispatches planning on that post). An empty
 *  harness choice sends nothing, and the daemon's own default stands. */
async function createIssue(projectId, goal, choice) {
  const created = await replyOrNothing(
    App.call("issue.create", {
      goal,
      project_id: projectId,
      dispatch: false,
      ...agentChoiceParams(catalog(), choice),
    }),
  );
  if (!created) return null;
  return { name: "issue", projectId: created.project_id || projectId, id: created.issue_id || created.plan_id };
}

/**
 * Open the create modal on `kind`'s tab, for the project named. `navigate` is
 * how the thing made is opened — the rail passes its own, which puts the rail
 * away on a narrow viewport. Returns { close }.
 */
export function openCreateWork({ projectId, projectName, kind = "branch", navigate = go }) {
  const state = {
    projectName: projectName || projectId,
    kind: CREATE_KINDS.includes(kind) ? kind : "branch",
    values: { branch: "", issue: "" },
    busy: false,
    error: "",
    choice: loadAgentDefaults(),
    choiceOpen: false,
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
      };
    });
    const input = body.querySelector("#create-work-input");
    const preview = body.querySelector("#create-work-preview");
    input.focus();
    input.setSelectionRange(caret, caret);
    input.oninput = () => {
      state.values[state.kind] = input.value;
      if (state.kind === "branch") preview.textContent = branchNamePreview(input.value);
    };
    input.onkeydown = (event) => {
      if (event.key === "Enter" && (state.kind === "branch" || event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        submit();
      }
    };
    body.querySelector("[data-create-cancel]").onclick = close;
    body.querySelector("[data-create-go]").onclick = submit;
    wireChoice();
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

  async function submit() {
    if (state.busy) return;
    const { kind } = state;
    const value = (state.values[kind] || "").trim();
    if (!value) {
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
      const route = kind === "branch" ? await createBranch(projectId, value) : await createIssue(projectId, value, state.choice);
      settle(kind, route);
    } catch (error) {
      state.busy = false;
      state.error = error.message || String(error);
      paint();
    }
  }

  /** The form is done with: shut it, re-read the board, and open what was made
   *  where the reply named it. */
  function settle(kind, route) {
    close();
    refreshFeed();
    if (!route) return;
    // A freshly cut branch has nobody in it yet — the rail opens on the ghost
    // composer, and that is exactly where typing the first message belongs.
    if (kind === "branch") App.focusComposerOnMount = true;
    navigate(route);
  }

  wire((state.values[state.kind] || "").length);
  return { close };
}
