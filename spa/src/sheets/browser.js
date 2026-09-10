// A host directory browser in the sheet. Optional callRpc and startPath scope
// browsing to a specific device and initially open its configured folder.
// gitOnly → only git repos are "Choose"-able and the footer adds the current
// repo; otherwise every folder is selectable (used to pick the projects dir).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";

export async function openBrowser(opts) {
  const callRpc = opts.callRpc || ((method, params) => App.call(method, params));
  const sheet = $("#sheet");
  let request = 0;
  const cancel = () => {
    request += 1;
    if (opts.onCancel) opts.onCancel();
    else $("#scrim").classList.remove("show");
  };
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
      <div class="browse-row">
        <button type="button" class="bname browse-nav" data-path="${esc(entry.path)}">${entry.is_git ? "📦" : "📁"} ${esc(entry.name)}</button>
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
        ${data.parent ? `<div class="browse-row"><button type="button" class="bname browse-nav dim" data-path="${esc(data.parent)}">⬆ up — parent folder</button></div>` : ""}
        ${rows || '<div class="dim" style="font-size:13px;padding:10px">No subfolders here.</div>'}
      </div>
      <div class="row">${footer}<button class="btn" id="bcancel" style="margin-left:auto">Cancel</button></div>
      <div class="adderr" id="berr"></div>`;
    $("#showhidden").onchange = (e) => {
      showHidden = e.target.checked;
      paint();
    };
    $("#bcancel").onclick = cancel;
    const chooseCurrent = $("#choosecur");
    if (chooseCurrent) chooseCurrent.onclick = () => opts.onChoose(chooseCurrent.dataset.path);
    $("#sheet").querySelectorAll(".browse-nav").forEach((row) => (row.onclick = () => nav(row.dataset.path)));
    $("#sheet").querySelectorAll(".use").forEach(
      (btn) =>
        (btn.onclick = (event) => {
          event.stopPropagation();
          opts.onChoose(btn.dataset.path);
        }),
    );
  };
  const nav = async (path) => {
    const version = ++request;
    const loading = sheet.firstElementChild;
    try {
      const next = await callRpc("fs.list", path ? { path } : {});
      if (version !== request || !loading.isConnected) return;
      data = next;
    } catch (e) {
      if (version !== request || !loading.isConnected) return;
      const err = $("#berr");
      if (err) err.textContent = e.message;
      sheet.querySelector(".browse-loading")?.remove();
      return;
    }
    paint();
  };
  $("#scrim").classList.add("show");
  sheet.innerHTML = `<h3>${esc(opts.title)}</h3><div class="dim browse-loading" style="padding:14px">loading…</div>
    <button class="btn" id="bcancel">Cancel</button><div class="adderr" id="berr" role="status"></div>`;
  $("#bcancel").onclick = cancel;
  await nav(opts.startPath || null);
}
