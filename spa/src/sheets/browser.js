// A host directory browser in the sheet. opts: { title, gitOnly, onChoose, onCancel }.
// gitOnly → only git repos are "Choose"-able and the footer adds the current
// repo; otherwise every folder is selectable (used to pick the projects dir).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";

export async function openBrowser(opts) {
  let data = null,
    showHidden = false;
  // Re-render the current folder applying the hidden filter — no re-fetch on toggle.
  const paint = () => {
    const visible = data.entries.filter((entry) => showHidden || !entry.is_hidden);
    const hiddenCount = data.entries.length - visible.length;
    // Every folder row opens on click; a "Use" button selects it (git repos in
    // repo mode, any folder in folder mode).
    const rows = visible
      .map(
        (entry) => `
      <div class="browse-row nav" data-path="${esc(entry.path)}">
        <span class="bname">${entry.is_git ? "📦" : "📁"} ${esc(entry.name)}</span>
        ${entry.is_git ? '<span class="bgit">git</span>' : ""}
        ${!opts.gitOnly || entry.is_git ? `<button class="btn primary mini use" data-path="${esc(entry.path)}">Use</button>` : ""}
      </div>`,
      )
      .join("");
    const footer =
      !opts.gitOnly || data.is_git
        ? `<button class="btn primary" id="choosecur" data-path="${esc(data.path)}">${opts.gitOnly ? "Use this repo" : "Use this folder"}</button>`
        : "";
    $("#sheet").innerHTML = `
      <h3>${esc(opts.title)}</h3>
      <div class="browse-path">${esc(data.path)}</div>
      <label class="toggle browse-toggle"><input type="checkbox" id="showhidden" ${showHidden ? "checked" : ""}> Show hidden${hiddenCount && !showHidden ? ` (${hiddenCount})` : ""}</label>
      <div class="browse-list">
        ${data.parent ? `<div class="browse-row nav" data-path="${esc(data.parent)}"><span class="bname dim">⬆ up — parent folder</span></div>` : ""}
        ${rows || '<div class="dim" style="font-size:13px;padding:10px">No subfolders here.</div>'}
      </div>
      <div class="row">${footer}<button class="btn" id="bcancel" style="margin-left:auto">Cancel</button></div>
      <div class="adderr" id="berr"></div>`;
    $("#showhidden").onchange = (e) => {
      showHidden = e.target.checked;
      paint();
    };
    $("#bcancel").onclick = () => (opts.onCancel ? opts.onCancel() : $("#scrim").classList.remove("show"));
    const chooseCurrent = $("#choosecur");
    if (chooseCurrent) chooseCurrent.onclick = () => opts.onChoose(chooseCurrent.dataset.path);
    $("#sheet").querySelectorAll(".browse-row.nav").forEach((row) => (row.onclick = () => nav(row.dataset.path)));
    $("#sheet").querySelectorAll(".use").forEach(
      (btn) =>
        (btn.onclick = (event) => {
          event.stopPropagation();
          opts.onChoose(btn.dataset.path);
        }),
    );
  };
  const nav = async (path) => {
    try {
      data = await App.call("fs.list", path ? { path } : {});
    } catch (e) {
      const err = $("#berr");
      if (err) err.textContent = e.message;
      return;
    }
    paint();
  };
  $("#scrim").classList.add("show");
  $("#sheet").innerHTML = `<h3>${esc(opts.title)}</h3><div class="dim" style="padding:14px">loading…</div>`;
  await nav(null);
}
