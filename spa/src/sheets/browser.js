// A host directory browser in the sheet. The caller it is opened with is the
// machine whose folders it shows, and startPath opens it at that machine's
// configured folder.
// gitOnly → only git repos are "Choose"-able and the footer adds the current
// repo; otherwise every folder is selectable (used to pick the projects dir).

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { readCached, subscribeCache, writeCached } from "../core/localCache.js";

export const browserListingAddress = (deviceId, path) => ({ deviceId, entityId: "fs-browser", kind: "listing", sub: path || "" });

const missingDirectory = (error) => /No such file or directory \(os error 2\)$/.test(error?.message || "");

async function listDirectory(callRpc, path, fallback, isCurrent) {
  try {
    return await callRpc("fs.list", path ? { path } : {});
  } catch (error) {
    if (!fallback || !missingDirectory(error) || !isCurrent()) throw error;
    return callRpc("fs.list", {});
  }
}

function bindCancel(container, cancel) {
  const button = container.querySelector("#bcancel");
  if (button) button.onclick = cancel;
}

function createDirectoryHtml(enabled) {
  return enabled
    ? `<form id="bmkdirform" class="row browse-create"><label class="sr-only" for="bdirname">New folder name</label><input id="bdirname" placeholder="New folder name" autocomplete="off"><button class="btn" id="bmkdir" type="submit">Create folder</button></form>`
    : "";
}

function bindCreateDirectory(container, createDirectory) {
  container.querySelector("#bmkdirform")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void createDirectory();
  });
}

function listingHtml(data, opts, showHidden, cancelHtml) {
  const visible = data.entries.filter((entry) => showHidden || !entry.is_hidden);
  const hiddenCount = data.entries.length - visible.length;
  const rows = visible.map((entry) => `
      <div class="browse-row">
        <button type="button" class="bname browse-nav" data-path="${esc(entry.path)}">${entry.is_git ? "📦" : "📁"} ${esc(entry.name)}</button>
        ${entry.is_git ? '<span class="bgit">git</span>' : ""}
        ${!opts.gitOnly || entry.is_git ? `<button class="btn primary mini use" data-path="${esc(entry.path)}">Use</button>` : ""}
      </div>`).join("");
  const footer = !opts.gitOnly || data.is_git
    ? `<button class="btn primary" id="choosecur" data-path="${esc(data.path)}">${opts.gitOnly ? "Use this repo" : "Use this folder"}</button>`
    : "";
  return `<div class="browse-path">${esc(data.path)}</div>
      <label class="toggle browse-toggle"><input type="checkbox" id="showhidden" ${showHidden ? "checked" : ""}> Show hidden${hiddenCount && !showHidden ? ` (${hiddenCount})` : ""}</label>
      <div class="browse-list">
        ${data.parent ? `<div class="browse-row"><button type="button" class="bname browse-nav dim" data-path="${esc(data.parent)}">⬆ up — parent folder</button></div>` : ""}
        ${rows || '<div class="dim" style="font-size:13px;padding:10px">No subfolders here.</div>'}
      </div>
      ${createDirectoryHtml(opts.allowCreateDirectory)}
      <div class="row">${footer}${cancelHtml}</div>
      <div class="adderr" id="berr"></div>`;
}

export async function openBrowser(opts) {
  const { callRpc } = opts;
  const deviceId = opts.deviceId || opts.cacheScope?.deviceId || "";
  const sheet = $("#sheet");
  const container = opts.container || sheet;
  const embedded = container !== sheet;
  const present = (bodyHtml) => embedded ? bodyHtml : settingsSheetHtml({ title: opts.title, bodyHtml });
  const cancelHtml = embedded ? "" : '<button class="btn" id="bcancel" style="margin-left:auto">Cancel</button>';
  let request = 0;
  let readRequest = 0;
  let cacheWrites = 0;
  let unwatch = null;
  let renderedRoot = null;
  let cancelled = false;
  const current = () => !cancelled && container.isConnected && container.firstElementChild === renderedRoot;
  const cancel = () => {
    request += 1;
    cancelled = true;
    unwatch?.();
    if (opts.onCancel) opts.onCancel();
    else $("#scrim").classList.remove("show");
  };
  let data = null,
    showHidden = false,
    creating = false;
  // Re-render the current folder applying the hidden filter — no re-fetch on toggle.
  const paint = () => {
    if (!data || !current()) return;
    container.innerHTML = present(listingHtml(data, opts, showHidden, cancelHtml));
    renderedRoot = container.firstElementChild;
    container.querySelector("#showhidden").onchange = (e) => {
      showHidden = e.target.checked;
      paint();
    };
    bindCancel(container, cancel);
    bindCreateDirectory(container, createDirectory);
    const chooseCurrent = container.querySelector("#choosecur");
    if (chooseCurrent) chooseCurrent.onclick = () => opts.onChoose(chooseCurrent.dataset.path);
    container.querySelectorAll(".browse-nav").forEach((row) => (row.onclick = () => nav(row.dataset.path)));
    container.querySelectorAll(".use").forEach(
      (btn) =>
        (btn.onclick = (event) => {
          event.stopPropagation();
          opts.onChoose(btn.dataset.path);
        }),
    );
  };
  const createDirectory = async () => {
    if (creating) return;
    const input = container.querySelector("#bdirname");
    const name = input.value.trim();
    if (!/^(?!\.{1,2}$)[^\\/]+$/.test(name)) {
      container.querySelector("#berr").textContent = "Use a single folder name without / or \\.";
      input.focus();
      return;
    }
    const version = ++request;
    const isCurrent = () => version === request && input.isConnected;
    creating = true;
    input.disabled = true;
    container.querySelector("#bmkdir").disabled = true;
    container.querySelector("#berr").textContent = "";
    try {
      const created = await callRpc("fs.mkdir", { parent: data.path, name });
      if (!isCurrent()) return;
      await nav(created.path);
    } catch (error) {
      if (isCurrent()) {
        container.querySelector("#berr").textContent = error.message;
        input.disabled = false;
        container.querySelector("#bmkdir").disabled = false;
        input.focus();
      }
    } finally {
      creating = false;
      const currentInput = container.querySelector("#bdirname");
      if (currentInput) currentInput.disabled = false;
      const currentButton = container.querySelector("#bmkdir");
      if (currentButton) currentButton.disabled = false;
    }
  };
  const nav = async (path, fallbackFromMissingStart = false) => {
    const version = ++request;
    const stale = () => version !== request || !current();
    unwatch?.();
    const address = browserListingAddress(deviceId, path);
    const readRecord = async () => {
      const reading = ++readRequest;
      const record = await readCached(address);
      if (stale() || reading !== readRequest) return;
      if (record?.value) {
        data = record.value;
        paint();
      }
    };
    unwatch = subscribeCache(address, () => {
      cacheWrites += 1;
      void readRecord();
    });
    const before = cacheWrites;
    const pulled = listDirectory(callRpc, path, fallbackFromMissingStart,
      () => !stale());
    void pulled.catch(() => {});
    await readRecord();
    if (stale()) return;
    try {
      const next = await pulled;
      if (stale() || cacheWrites !== before) return;
      await writeCached(address, next);
      await readRecord();
    } catch (e) {
      if (stale()) return;
      const err = container.querySelector("#berr");
      if (err) err.textContent = e.message;
      container.querySelector(".browse-loading")?.remove();
      return;
    }
  };
  $("#scrim").classList.add("show");
  container.innerHTML = present(`<div class="dim browse-loading" style="padding:14px">loading…</div>
    ${cancelHtml}<div class="adderr" id="berr" role="status"></div>`);
  renderedRoot = container.firstElementChild;
  bindCancel(container, cancel);
  await nav(opts.startPath || null, Boolean(opts.startPath && opts.fallbackFromMissingStart));
}
