// Filing an issue without leaving the list.
//
// #57: "Issue creation should be inline and support file attachments."
// It was a modal over a dialog over the tab: you left the list to file against
// it, and you could not attach the screenshot that was the reason you were
// filing at all. So the form opens IN the tab, at the head of the list it is
// about, and it takes files the way the chat composer takes them.
//
// ## Mounted once, like everything else on this bar
//
// The composer is made when it opens and is the same DOM until it closes. That
// is not a style rule — it is the only way it can work. A title half typed, a
// caret in the body, three files going up in the tray and a menu the reader
// has open are all state that lives in the DOM, and the Issues tab repaints
// whenever a push says an issue moved. So nothing here re-renders on a paint:
// the title, the body, the tray and the three menus are written once, and the
// only zone that is ever redrawn is the assignee's, because choosing a kind
// that cuts a workspace grows two more fields.
//
// ## What it reuses rather than reimplements
//
// `mountComposerAttachments` (core/composer.js) — the paperclip, the paste, the
// drop, the tray, the per-chip failure. It asks only for a handful of ids and
// an `upload`, so this renders the shape it needs and hands it a project-scoped
// upload instead of a conversation-scoped one.
//
// `mountFilterMenu` (core/filterMenuControl.js) — column, priority and labels,
// so the form reads as the bar above it. Labels are the one that also INVENTS:
// a filter chooses from what is, and a composer has to be able to name a label
// nobody has used yet.
//
// The assignee keeps its own control (core/trackerAssigneeControl.js) and is
// deliberately not a filter menu: two of its five kinds open forms, and one of
// them cuts a checkout. Filing an issue and starting an agent on a new
// workspace in one press is the thing this tracker has that GitHub's does not,
// and a pick-one-of-a-list cannot say it.

import { esc, messageOf } from "./text.js";
import { ICON_PAPERCLIP } from "./icons.js";
import { composerPartIds, autoGrow, mountComposerAttachments } from "./composer.js";
import { mountFilterMenu } from "./filterMenuControl.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { confirmAction } from "./confirm.js";
import { PRIORITIES, columnsOf } from "./trackerModel.js";
import { assignRefusalText } from "./trackerAssignee.js";
import {
  assigneeControlHtml,
  draftAssignee,
  draftStartsWork,
  draftWaitsOnAWorkspace,
  emptyAssigneeDraft,
  wireAssigneeControl,
} from "./trackerAssigneeControl.js";
import { fieldTraits } from "./fieldTraits.js";

const PREFIX = "issue-new";
const INPUT_ID = `${PREFIX}-body`;

/**
 * What a priority menu offers.
 *
 * Not `none`: an unset menu already means none, and offering it as a row would
 * be two ways to say one thing — the menu's own Clear is the way back. What
 * that buys is a press that reads as the FIELD until it is answered ("Priority",
 * then "High"), which is how the column beside it reads too.
 */
export const priorityMenuOptions = () =>
  PRIORITIES.filter((priority) => priority.id !== "none").map((priority) => ({
    value: priority.id,
    label: priority.label,
  }));

/** What a column menu offers. Every column, and no "any": an issue is filed
 *  INTO one, and the verb's own default is the first. */
export const columnMenuOptions = (columns) =>
  columnsOf(columns).map((column) => ({ value: column.id, label: column.name }));

/** What a label menu offers: every label the project's issues already wear,
 *  plus whatever this draft has invented, so a coined label stays on the list
 *  it was coined into. */
export const labelMenuOptions = (known, chosen) => {
  const named = new Set((known || []).map((label) => label.value ?? label));
  const rows = (known || []).map((label) => (label.value === undefined ? { value: label, label } : label));
  for (const one of chosen || []) {
    if (!named.has(one)) rows.push({ value: one, label: one });
  }
  return rows.filter((row) => row.value !== "");
};

/** A field, or nothing at all. Nothing is what the verb's own defaults are
 *  said with: `issues.create` reads an absent `status` as Backlog and an
 *  absent `priority` as none, and an empty string is not absent. */
const said = (key, value) => (value ? { [key]: value } : null);

/** The one value a single-choice menu holds. */
const only = (chosen) => (chosen?.length ? chosen[0] : "");

/** The form as `issues.create` params. A field nobody filled in is left off:
 *  the verb's own defaults are the record's defaults, and sending an empty
 *  string instead would store one. */
export function composedIssueParams(draft, { projectId, assignee, attachments }) {
  return {
    project_id: projectId,
    title: String(draft.title || "").trim(),
    ...said("body", String(draft.body || "").trim() && draft.body),
    ...said("labels", draft.labels?.length && draft.labels),
    ...said("priority", only(draft.priority)),
    ...said("status", only(draft.status)),
    ...said("assignee", assignee),
    ...said("attachments", attachments?.length && attachments),
  };
}

/**
 * Whether the files this form sent went nowhere.
 *
 * The same trap a dropped assignee falls into, and for the same reason: a v1
 * handler parses params into its own struct and serialises THAT back before
 * the implementation reads them, so a field the bridge predates is dropped at
 * the facade rather than refused (`api/v1/mod.rs`). Filing with attachments on
 * a bridge whose `issues.create` does not take them therefore answers ok, with
 * a filed issue carrying no files and nothing in the answer that says so.
 *
 * "I attached the screenshot and it is not there" is worth a sentence, because
 * the screenshot was usually the reason for filing.
 */
export const attachmentsWentNowhere = (sent, answer) =>
  Boolean(sent?.length) && !(answer?.issue?.attachments?.length);

/** Whether closing now would throw anything away. A composer with no tray has
 *  nothing in one, which is why the tray is asked rather than assumed. */
export const composerHasContent = (draft, attachments) =>
  Boolean(
    String(draft.title || "").trim() ||
      String(draft.body || "").trim() ||
      draft.labels?.length ||
      (attachments && !attachments.isEmpty()),
  );

const ids = composerPartIds(INPUT_ID);
const savedDraftFields = (saved) => ({
  title: saved.title,
  body: saved.body || "",
  status: saved.status || [],
  priority: saved.priority || [],
  labels: saved.labels || [],
});

/// The body box, in the shape `mountComposerAttachments` reads: it asks for
/// `.composer`, the tray, the textarea, the hidden file input and the
/// paperclip, and wires the paste and the drop onto the root it is handed. The
/// LAYOUT is ours — a send arrow belongs on a message, not on a form.
///
/// A bridge that cannot carry files on an issue gets the plain box: no
/// paperclip, no tray, no drop mask, and nothing mounted over them. An
/// affordance that is drawn and then apologised for is worse than one that was
/// never offered (core/issueAttachments.js).
const bodyBoxHtml = (attachable) => `${attachable ? `<div class="composer-tray" id="${ids.tray}" hidden></div>` : ""}
  <div class="composer${attachable ? " attachable" : ""} issue-compose-body">
    <textarea id="${INPUT_ID}" rows="3" ${fieldTraits("prose")} placeholder="Anything the title leaves out (markdown)"></textarea>
    <div class="composer-bar">
      <span class="hint issue-compose-hint"></span>
      <div class="composer-actions">
        ${attachable ? `<input type="file" id="${ids.file}" class="composer-file" multiple hidden>
        <button type="button" class="composer-attach" id="${ids.attach}" aria-label="Attach files" title="Attach files">${ICON_PAPERCLIP}</button>` : ""}
      </div>
    </div>
    ${attachable ? `<div class="composer-dropmask" aria-hidden="true"><span>Drop to attach</span></div>` : ""}
  </div>`;

const frameHtml = (projectName, attachable) => `<section class="issue-compose" aria-label="File an issue in ${esc(projectName)}">
    <input class="issue-compose-title" id="${PREFIX}-summary" type="text" ${fieldTraits("line", "next")}
      placeholder="What should be done" aria-label="Title">
    ${bodyBoxHtml(attachable)}
    <div class="issue-compose-facets"></div>
    <div class="issue-compose-assignee"></div>
    <p class="warn issue-compose-error" hidden></p>
    <p class="sub issue-compose-waiting" role="status" hidden>Filing the issue, cutting the checkout and starting the agent. This can take a minute on a large repository.</p>
    <div class="issue-compose-row">
      <button class="btn" type="button" data-compose-cancel>Cancel</button>
      <button class="btn primary" type="button" data-compose-file>File issue</button>
    </div>
  </section>`;

/** The three menus, in the order a reader fills them. */
const FACETS = [
  { name: "status", label: "Column", multi: false },
  { name: "priority", label: "Priority", multi: false },
  { name: "labels", label: "Labels", multi: true, summary: "count", invent: (word) => `Create “${word}”` },
];

/**
 * Open the composer into `host`.
 *
 * `attachable` is whether this device's bridge can carry files on an issue; a
 * form that is not gets no paperclip rather than one that apologises.
 * `onFiled(answer, outcome)` is handed the whole answer and what the answer did
 * not say for itself. `onClosed` runs however it ends, so the caller can put
 * the focus back where the reader left it.
 */
export function openIssueComposer(host, {
  projectId,
  deviceId = "",
  projectName,
  columns,
  labels = [],
  options,
  catalog = null,
  attachable = false,
  callRpc,
  onFiled = null,
  onClosed = null,
}) {
  host.innerHTML = frameHtml(projectName || projectId || "this project", attachable);
  const root = host.querySelector(".issue-compose");
  const title = root.querySelector(`#${PREFIX}-summary`);
  const body = root.querySelector(`#${INPUT_ID}`);
  const facets = root.querySelector(".issue-compose-facets");
  const assigneeHost = root.querySelector(".issue-compose-assignee");
  const errorLine = root.querySelector(".issue-compose-error");
  const waitingLine = root.querySelector(".issue-compose-waiting");
  const filePress = root.querySelector("[data-compose-file]");
  const cancelPress = root.querySelector("[data-compose-cancel]");

  const draft = { title: "", body: "", status: [], priority: [], labels: [] };
  const state = { draft: emptyAssigneeDraft("none"), catalog, busy: false, closed: false };
  const draftAddress = uiAddress({ deviceId, entityId: projectId, view: "issue-composer", kind: "draft" });
  const snapshot = () => ({ ...draft, assignee: state.draft });
  const draftRecord = watchUiState(draftAddress, (saved) => {
    if (state.closed || !saved || typeof saved.title !== "string") return;
    Object.assign(draft, savedDraftFields(saved));
    state.draft = saved.assignee || state.draft;
    if (title.value !== draft.title) title.value = draft.title;
    if (body.value !== draft.body) body.value = draft.body;
    paintMenus();
    paintAssignee();
    paintPress();
    autoGrow(body);
  }, { debounceMs: 180 });
  const saveDraft = (debounced = false) => {
    if (debounced) draftRecord.schedule(snapshot());
    else void draftRecord.write(snapshot());
  };

  // ---- the parts that are made once ----------------------------------------

  // `let`, and the paints below are declarations rather than consts: mounting
  // the tray calls `onChange` while it is still being mounted, so everything it
  // can reach has to exist before the mount rather than after it.
  let attachments = null;
  if (attachable) {
    attachments = mountComposerAttachments(root, {
      ids: { input: INPUT_ID },
      upload: (file, base64) => upload(file, base64),
      onError: (message) => say(message),
      onChange: () => paintPress(),
    });
  }

  const menus = new Map(
    FACETS.map((facet) => [
      facet.name,
      mountFilterMenu(facets, {
        ...facet,
        cacheAddress: uiAddress({ deviceId, entityId: projectId, view: "issue-composer", kind: "menu", sub: facet.name }),
        onChange: (chosen) => {
          draft[facet.name] = chosen;
          saveDraft();
          paintMenus();
          paintPress();
        },
      }),
    ]),
  );

  function paintMenus() {
    menus.get("status").update(columnMenuOptions(columns), draft.status);
    menus.get("priority").update(priorityMenuOptions(), draft.priority);
    menus.get("labels").update(labelMenuOptions(labels, draft.labels), draft.labels);
  }

  // ---- the one zone that is ever redrawn -----------------------------------

  /// Choosing a kind that cuts a workspace grows two more fields, so this zone
  /// is re-rendered when the SHAPE changes and never merely because somebody
  /// typed in it — a form that rebuilds itself under a caret is a form you
  /// cannot type a workspace name into.
  const paintAssignee = () => {
    assigneeHost.innerHTML = assigneeControlHtml(options, state.draft, { prefix: PREFIX, catalog: state.catalog });
    wireAssigneeControl(assigneeHost, state.draft, {
      prefix: PREFIX,
      onDraft: (next) => {
        const reshaped = next.optionId !== state.draft.optionId || next.choiceOpen !== state.draft.choiceOpen;
        state.draft = next;
        saveDraft(reshaped === false);
        if (reshaped) paintAssignee();
        paintPress();
      },
    });
  };

  // ---- what the form says about itself -------------------------------------

  const waitingOnAWorkspace = () => draftWaitsOnAWorkspace(options, state.draft);

  function pressLabel() {
    if (state.busy) return waitingOnAWorkspace() ? "cutting the workspace…" : "filing…";
    if (attachments?.busy()) return "attaching…";
    return draftStartsWork(options, state.draft) ? "File and start" : "File issue";
  }

  function paintPress() {
    filePress.textContent = pressLabel();
    filePress.disabled = state.busy || Boolean(attachments?.busy());
    cancelPress.disabled = state.busy;
    title.disabled = state.busy;
    body.disabled = state.busy;
    waitingLine.hidden = !(state.busy && waitingOnAWorkspace());
  }

  function say(message) {
    errorLine.textContent = message || "";
    errorLine.hidden = !message;
  }

  // ---- filing --------------------------------------------------------------

  async function upload(file, base64) {
    return callRpc("issues.attach", { project_id: projectId, filename: file.name, content_b64: base64 });
  }

  /** Ready to be filed, or the reason it is not. */
  function refuses() {
    if (state.busy || state.closed) return "busy";
    draft.title = title.value;
    draft.body = body.value;
    if (draft.title.trim()) return "";
    say("An issue needs a title.");
    title.focus();
    return "untitled";
  }

  /** What the answer did not say for itself. Both of these are silent
   *  droppings at the v1 facade rather than refusals, so both are read against
   *  what was sent rather than looked for in the answer. */
  const outcomeOf = (assignee, sent, answer) => ({
    assigneeWentNowhere: Boolean(assignee) && !answer?.issue?.assignee,
    attachmentsWentNowhere: attachmentsWentNowhere(sent, answer),
    attachmentCount: sent.length,
  });

  async function file() {
    if (refuses()) return;
    state.busy = true;
    say("");
    paintPress();
    const sent = attachments?.attachments() || [];
    const assignee = draftAssignee(options, state.draft, state.catalog);
    try {
      const params = composedIssueParams(draft, { projectId, assignee, attachments: sent });
      const answer = await callRpc("issues.create", params);
      if (state.closed) return;
      await draftRecord.write({ title: "", body: "", status: [], priority: [], labels: [], assignee: emptyAssigneeDraft("none") });
      close({ filed: true });
      onFiled?.(answer, outcomeOf(assignee, sent, answer));
    } catch (error) {
      if (state.closed) return;
      state.busy = false;
      say(assignRefusalText(messageOf(error)));
      paintPress();
    }
  }

  function close({ filed = false } = {}) {
    if (state.closed) return;
    state.closed = true;
    draftRecord.dispose();
    menus.forEach((menu) => menu.dispose());
    host.innerHTML = "";
    onClosed?.({ filed });
  }

  /// Escape throws the draft away, so it asks first — but only when there is a
  /// draft to throw. An empty form shuts on the first press, because making
  /// somebody confirm that they typed nothing is the confirm nobody reads.
  async function cancel() {
    if (state.busy) return;
    if (composerHasContent(draft, attachments)) {
      const sure = await confirmAction({
        title: "Throw this issue away?",
        intro: "Nothing has been filed. The title, the description and anything attached go with it.",
        confirmLabel: "Discard",
        cancelLabel: "Keep writing",
        danger: true,
      });
      if (!sure || state.closed) return;
    }
    await draftRecord.write({ title: "", body: "", status: [], priority: [], labels: [], assignee: emptyAssigneeDraft("none") });
    close();
  }

  // ---- the keyboard --------------------------------------------------------

  const files = (event) => (event.metaKey || event.ctrlKey) && event.key === "Enter";

  title.onkeydown = (event) => {
    draft.title = title.value;
    if (files(event)) {
      event.preventDefault();
      void file();
      return;
    }
    // Enter in a one-line field means "I am done with this line", and the next
    // line is the description — not a send. A title is rarely the whole issue.
    if (event.key === "Enter") {
      event.preventDefault();
      body.focus();
    }
  };
  title.oninput = () => {
    draft.title = title.value;
    saveDraft(true);
  };
  body.oninput = () => {
    draft.body = body.value;
    saveDraft(true);
  };
  body.onkeydown = (event) => {
    if (!files(event)) return;
    event.preventDefault();
    void file();
  };
  root.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    void cancel();
  });

  filePress.onclick = () => void file();
  cancelPress.onclick = () => void cancel();

  paintMenus();
  paintAssignee();
  paintPress();
  autoGrow(body);
  title.focus();

  return {
    close,
    /// What the caller's own repaint must not disturb. The pane asks this
    /// before it decides whether a push may take the composer away.
    isOpen: () => !state.closed,
  };
}
