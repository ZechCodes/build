import { esc } from "../core/text.js";

const states = {
  waiting: { label: "Waiting", mark: "○" },
  in_progress: { label: "In progress", mark: "●" },
  done: { label: "Done", mark: "✓" },
  failed: { label: "Failed", mark: "!" },
  unknown: { label: "Outcome unknown", mark: "?" },
};
const descriptions = {
  in_progress: "Creating project",
  done: "Project created. Opening project…",
  failed: "Project could not be created. Go back to edit the folders and try again.",
};

function sourceHtml(source, state, error) {
  const action = source.remote ? `Cloning ${source.remote}` : `Adding ${source.path}`;
  const { label, mark } = states[state];
  return `<li class="nr-progress-source" data-source-state="${state}">
    <span class="nr-progress-mark" aria-hidden="true">${mark}</span>
    <div class="nr-progress-detail"><div>${esc(action)} as ${esc(source.name)}</div>
      <div class="nr-progress-state">${label}</div>
      ${error ? `<div class="adderr">${esc(error)}</div>` : ""}</div></li>`;
}

function paintActions(actions, state, { onBack, onClose }) {
  const pending = state === "in_progress";
  actions.setAttribute("aria-busy", String(pending));
  actions.hidden = pending;
  actions.innerHTML = pending ? "" : `<button class="btn" id="nrcancel" type="button" style="margin-left:auto">${state === "failed" ? "Cancel" : "Close"}</button>${state === "failed" ? '<button class="btn primary" id="nrback" type="button">Back</button>' : ""}`;
  const back = actions.querySelector("#nrback");
  if (back) back.onclick = onBack;
  const close = actions.querySelector("#nrcancel");
  if (close) close.onclick = onClose;
}

/** The bridge replies once for the whole creation. Keep all source lines in
 * step until it can publish individual source progress. No timer invents it. */
export function mountNewRepoProgress(sheet, { name, sources }, { onBack, onClose, deviceName }) {
  // Only the stable summary is live. Busy actions are its sibling, and source
  // URLs are outside its atomic region so an opening error does not reread them.
  sheet.innerHTML = `<section id="nrprogress" tabindex="-1"><h3>${esc(name)}</h3>
    <p class="sub" id="nrprogress-status" role="status" aria-live="polite" aria-atomic="true"></p>
    <ul class="nr-progress-sources"></ul><p class="sub nr-progress-note" id="nrprogress-note"></p>
    <p class="adderr" id="nropenerr" hidden></p><p class="sub" id="nrlosterr" hidden></p>
    </section><div class="row" id="nrprogress-actions"></div>`;
  sheet.scrollTop = 0;
  const root = sheet.querySelector("#nrprogress");
  const actions = sheet.querySelector("#nrprogress-actions");
  const status = root.querySelector("#nrprogress-status");
  const list = root.querySelector(".nr-progress-sources");
  const note = root.querySelector("#nrprogress-note");
  const update = (state, error = "", summary = descriptions[state]) => {
    status.textContent = summary;
    list.innerHTML = sources.map((source) => sourceHtml(source, state, error)).join("");
    note.hidden = state !== "in_progress";
    note.textContent = "Large repositories can take a while to clone.";
    paintActions(actions, state, { onBack, onClose });
  };
  const openingFailed = (error) => {
    const detail = root.querySelector("#nropenerr");
    detail.textContent = error;
    detail.hidden = false;
    status.textContent = "Project created. Could not open it.";
  };
  const unknown = (error) => {
    update("unknown", "", `The creation request was lost. The project may still be being created. If it succeeds, it will appear in the Projects list on ${deviceName}. Close this sheet and check the list before creating it again.`);
    const detail = root.querySelector("#nrlosterr");
    detail.textContent = error;
    detail.hidden = false;
  };
  update("in_progress");
  root.focus();
  return {
    root, done: () => update("done"), fail: (error) => update("failed", error),
    openingFailed, unknown,
  };
}
