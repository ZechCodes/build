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
// Everything a tab does differently — its field, what sits under it, what it
// preloads, what typing and the arrow keys mean there, and what pressing
// Create asks for — is one entry in CREATE_TABS. Adding a tab is adding an
// entry; nothing below ever asks which tab it is on.
//
// Nothing is dispatched by either create: the first message sent there is what
// starts an agent.

import { esc } from "./text.js";
import { App, go } from "../app.js";
import { refreshFeed } from "./taskFeed.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import { agentChoiceParams, agentChoicePanelHtml, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import { NOTHING_HIGHLIGHTED, branchPickerRows, nextHighlight, pressedRow } from "./branchPickerModel.js";
import { modalDialogHtml, openModal } from "./modal.js";

/** The two things a project can hold, in the order the tabs offer them. */
export const CREATE_KINDS = ["branch", "issue"];

/** Names the form's three harness controls, so its panel and compose's can be
 *  open at once without either answering for the other. */
const CHOICE_PREFIX = "create-choice";

const LEADING_ROW = 0;

const catalog = () => App.modelCatalog || { providers: [] };

/** What a tab's preload holds before it has been asked for: requested and
 *  done are two facts, and until the answer is in the rows have nothing to say. */
const NOTHING_LOADED = { requested: false, done: false, rows: [], error: "" };

/** What the Branch tab has preloaded, which is the project's branch listing. */
const branchListing = (state) => (state.loaded && state.loaded.branch) || NOTHING_LOADED;

/** The rows the Branch tab is showing for what has been typed. Read off state
 *  wherever they are needed, so the painted list and the pressed row can never
 *  be two different answers. */
const pickerRows = (state) =>
  branchPickerRows({ projectId: state.projectId, branches: branchListing(state).rows, query: state.values.branch || "" });

/** The row pressing Create would take, which is also the branch the field is
 *  promising — one answer, so the preview cannot say one thing while Enter
 *  does another. */
const pressedAction = (state) =>
  pressedRow({ rows: pickerRows(state), query: state.values.branch || "", highlight: state.highlight });

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
 *  are none — which is only ever said once the listing is in. A listing that
 *  could not be read never stops a branch being cut by name, so it is a note
 *  beside the field and not the form's error. */
function branchPickerHtml(state) {
  const rows = pickerRows(state);
  const listing = branchListing(state);
  const note = listing.error
    ? `<div class="sub branch-picker-note">Couldn't list branches: ${esc(listing.error)}</div>`
    : rows.length || !listing.done
      ? ""
      : `<div class="sub branch-picker-note">No branch matches.</div>`;
  return `<div class="branch-picker" role="listbox" aria-label="Branches">${rows
    .map((row, index) => branchRowHtml(row, index, state.highlight))
    .join("")}${note}</div>`;
}

/** The project's branches, and the reason there are none to show. A listing
 *  that could not be read still leaves the field able to cut a branch by name,
 *  so the failure is an answer here and not a throw. */
async function readBranches(projectId) {
  try {
    const listing = await App.call("git.branches", { project_id: projectId });
    return { rows: (listing && listing.branches) || [], error: "" };
  } catch (error) {
    return { rows: [], error: error.message || String(error) };
  }
}

/** Filing an issue: inert by contract — the record exists, and nothing runs
 *  behind it until the first message (the bridge dispatches planning on that
 *  post). An empty harness choice sends nothing, and the daemon's own default
 *  stands. */
const issueAction = (projectId, goal, choice) => ({
  call: {
    method: "issue.create",
    params: { goal, project_id: projectId, dispatch: false, ...agentChoiceParams(catalog(), choice) },
  },
  land: (answer) => ({
    route: { name: "issue", projectId: answer.project_id || projectId, id: answer.issue_id || answer.plan_id },
    focusComposer: false,
  }),
});

/**
 * What each tab is: the words it asks in, the field it asks with, what sits
 * under that field, what it preloads, what typing and the keys mean there, and
 * what pressing Create asks for.
 *
 * Every entry is a pure description of its tab: it answers, and the modal's
 * own closure is the only thing that writes the modal's state. `action`
 * answers the whole pressable thing — a call and where its answer lands — or
 * null while the tab is asking for nothing. `onTyped` answers the highlight
 * the typed text asks for, or null where the tab has no highlight to move.
 * `load` answers what the tab preloads as { rows, error }, or is null where it
 * preloads nothing. `handleKey` answers whether the key was the tab's, and
 * `controls` is what a tab's keys can do: move its highlight, or submit.
 */
const CREATE_TABS = {
  branch: {
    tab: "Branch",
    hint: "Start work on any branch of the project, or name a new one. Nothing is dispatched — the first message you send starts an agent there.",
    label: "Branch",
    empty: "Name it first.",
    fieldHtml: (value) =>
      `<input id="create-work-input" type="text" class="path" placeholder="Find a branch, or name a new one…" autocomplete="off" value="${esc(value)}" />`,
    underHtml: branchPickerHtml,
    previewHtml: (state) => esc(pressedAction(state)?.name || ""),
    action: pressedAction,
    onTyped: (value) => (value.trim() ? LEADING_ROW : NOTHING_HIGHLIGHTED),
    handleKey: (event, controls) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        controls.moveHighlight(event.key === "ArrowDown" ? 1 : -1);
        return true;
      }
      if (event.key !== "Enter") return false;
      controls.submit();
      return true;
    },
    load: readBranches,
  },
  issue: {
    tab: "Issue",
    hint: "Say what you want. No planning agent starts until you send the first message.",
    label: "Goal",
    empty: "Describe the issue first.",
    fieldHtml: (value) =>
      `<textarea id="create-work-input" rows="3" placeholder="e.g. Add a /health endpoint that returns build SHA and uptime…">${esc(value)}</textarea>`,
    underHtml: (state) => agentChoicePanelHtml(catalog(), state.choice, { prefix: CHOICE_PREFIX, open: state.choiceOpen }),
    previewHtml: () => "",
    action: (state) => {
      const goal = (state.values.issue || "").trim();
      return goal ? issueAction(state.projectId, goal, state.choice) : null;
    },
    onTyped: () => null,
    handleKey: (event, controls) => {
      if (event.key !== "Enter" || !(event.metaKey || event.ctrlKey)) return false;
      controls.submit();
      return true;
    },
    load: null,
  },
};

/** The dialog's inside. `state`: { projectId, projectName, kind, values, busy,
 *  error, choice, choiceOpen, loaded, highlight }, where `loaded` holds what
 *  each tab has preloaded by its kind: { requested, done, rows, error }.
 *  Everything user-supplied is escaped. */
export function createWorkBodyHtml(state) {
  const tab = CREATE_TABS[state.kind];
  const value = state.values[state.kind] || "";
  const tabs = CREATE_KINDS.map(
    (kind) =>
      `<button class="btn seg${kind === state.kind ? " primary" : ""}" type="button" role="tab" aria-selected="${
        kind === state.kind ? "true" : "false"
      }" data-create-tab="${kind}">${CREATE_TABS[kind].tab}</button>`,
  ).join("");
  return `<h3>New in ${esc(state.projectName)}</h3>
    <div class="segmented create-tabs" role="tablist">${tabs}</div>
    <div class="sub create-hint">${esc(tab.hint)}</div>
    <label class="create-label" for="create-work-input">${esc(tab.label)}</label>
    ${tab.fieldHtml(value)}
    ${tab.underHtml(state)}
    <div class="warn create-error"${state.error ? "" : " hidden"}>${esc(state.error)}</div>
    <div class="row create-row">
      <span class="dim mono create-preview" id="create-work-preview">${tab.previewHtml(state)}</span>
      <button class="btn" data-create-cancel type="button">Cancel</button>
      <button class="btn primary" data-create-go type="button"${state.busy ? " disabled" : ""}>${state.busy ? "creating…" : "Create"}</button>
    </div>`;
}

export function createWorkHtml(state) {
  return modalDialogHtml(createWorkBodyHtml(state), { className: "modal-create" });
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
    loaded: {},
    highlight: NOTHING_HIGHLIGHTED,
  };
  let dismissed = false;
  const dismiss = () => {
    dismissed = true;
    return close();
  };
  const { body, close } = openModal({ dialogHtml: createWorkHtml(state), scrimId: "create-scrim", onClose: dismiss });
  const controls = { moveHighlight, submit };

  /** What the tab being looked at preloads, asked for once per tab. An answer
   *  that lands after the modal was dismissed is kept off the closing dialog. */
  async function preload() {
    const { load } = CREATE_TABS[state.kind];
    if (!load || !state.projectId) return;
    if (!state.loaded[state.kind]) state.loaded[state.kind] = { ...NOTHING_LOADED };
    const slot = state.loaded[state.kind];
    if (slot.requested) return;
    slot.requested = true;
    const { rows, error } = await load(state.projectId);
    slot.rows = rows;
    slot.error = error;
    slot.done = true;
    if (dismissed) return;
    paint();
  }

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
    body.querySelectorAll("[data-create-tab]").forEach((button) => {
      button.onclick = () => {
        state.kind = button.dataset.createTab;
        state.error = "";
        paint();
        preload();
      };
    });
    const input = body.querySelector("#create-work-input");
    input.focus();
    input.setSelectionRange(caret, caret);
    input.oninput = () => {
      state.values[state.kind] = input.value;
      const highlight = CREATE_TABS[state.kind].onTyped(input.value);
      if (highlight === null) return;
      state.highlight = highlight;
      paint();
    };
    input.onkeydown = (event) => {
      if (CREATE_TABS[state.kind].handleKey(event, controls)) event.preventDefault();
    };
    body.querySelectorAll("[data-branch-pick]").forEach((row) => {
      row.onclick = () => {
        state.highlight = Number(row.dataset.branchPick);
        return submit();
      };
    });
    body.querySelector("[data-create-cancel]").onclick = dismiss;
    body.querySelector("[data-create-go]").onclick = submit;
    wireChoice();
  }

  function moveHighlight(step) {
    state.highlight = nextHighlight({ rows: pickerRows(state), highlight: state.highlight, step });
    paint();
    const row = state.highlight >= 0 ? body.querySelector(`[data-branch-pick="${state.highlight}"]`) : null;
    if (row && row.scrollIntoView) row.scrollIntoView({ block: "nearest" });
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
    const tab = CREATE_TABS[state.kind];
    const action = tab.action(state);
    if (!action) {
      state.error = tab.empty;
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
    let answer = null;
    try {
      answer = action.call ? await App.call(action.call.method, action.call.params) : null;
    } catch (error) {
      state.busy = false;
      state.error = error.message || String(error);
      paint();
      return;
    }
    const { route, focusComposer } = action.land(answer);
    close();
    refreshFeed();
    App.focusComposerOnMount = focusComposer;
    navigate(route);
  }

  wire((state.values[state.kind] || "").length);
  preload();
  return { close };
}
