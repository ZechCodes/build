import { esc } from "../core/text.js";

const states = {
  waiting: { label: "Waiting", mark: "○" },
  in_progress: { label: "In progress", mark: "●" },
  done: { label: "Done", mark: "✓" },
  failed: { label: "Failed", mark: "!" },
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

/** The bridge replies once for the whole creation. Keep all source lines in
 * step until it can publish individual source progress. No timer invents it. */
export function mountNewRepoProgress(sheet, { name, sources }, { onBack, onClose }) {
  sheet.innerHTML = '<section id="nrprogress" role="status" aria-live="polite" aria-atomic="true" tabindex="-1"></section><div class="row" id="nrprogress-actions"></div>';
  sheet.scrollTop = 0;
  const root = sheet.querySelector("#nrprogress");
  const actions = sheet.querySelector("#nrprogress-actions");
  const update = (state, error = "") => {
    root.setAttribute("aria-busy", String(state === "in_progress"));
    root.innerHTML = `<h3>${esc(name)}</h3><p class="sub">${descriptions[state]}</p>
      <ul class="nr-progress-sources">${sources.map((source) => sourceHtml(source, state, error)).join("")}</ul>
      ${state === "in_progress" ? '<p class="sub nr-progress-note">Large repositories can take a while to clone.</p>' : ""}`;
    actions.innerHTML = state === "in_progress" ? "" : `<button class="btn" id="nrcancel" type="button" style="margin-left:auto">Cancel</button>${state === "failed" ? '<button class="btn primary" id="nrback" type="button">Back</button>' : ""}`;
    actions.hidden = state === "in_progress";
    const back = actions.querySelector("#nrback");
    if (back) back.onclick = onBack;
    const cancel = actions.querySelector("#nrcancel");
    if (cancel) cancel.onclick = onClose;
  };
  update("in_progress");
  root.focus();
  return { root, done: () => update("done"), fail: (error) => update("failed", error) };
}
