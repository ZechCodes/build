import { esc } from "./text.js";
import { refLabel } from "./workspaceModel.js";
import { directoryCacheId } from "./directoryScope.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";

const kindLabel = (kind) => kind === "tag" ? "Tags" : "Branches";

function syncBadge(ref) {
  const ahead = Number(ref.ahead) || 0;
  const behind = Number(ref.behind) || 0;
  if (ref.remote) return `<span class="workspace-refremote">Remote · ${esc(ref.remote)}</span>`;
  if (ref.upstream && behind > 0 && ahead === 0) return `<span class="workspace-refpull">Pull ${behind}</span>`;
  if (ref.upstream && behind > 0 && ahead > 0) return `<span class="workspace-refdiverged">Diverged · ${ahead}↑ ${behind}↓</span>`;
  return "";
}

function rowHtml(ref) {
  return `<button type="button" class="workspace-refrow" role="option" aria-selected="${Boolean(ref.current)}" data-ref="${esc(ref.full_ref)}">
    <span class="workspace-refname">${esc(ref.name)}</span>${ref.current ? '<span class="workspace-refcurrent">Current</span>' : ""}${syncBadge(ref)}
  </button>`;
}

export function mountWorkspaceRefPicker(host, { scope, callRpc, cacheScope, onCheckout } = {}) {
  host.innerHTML = `<div class="workspace-refpicker">
    <button type="button" class="workspace-reftrigger" data-refpicker-toggle aria-haspopup="listbox" aria-expanded="false" disabled>
      <span class="workspace-reftrigger-kind">Branch</span><span class="workspace-reftrigger-name">Loading refs…</span><span aria-hidden="true">⌄</span>
    </button>
    <div class="workspace-refmenu" hidden>
      <div class="workspace-reftabs" role="tablist" aria-label="Reference kind">
        <button type="button" role="tab" data-ref-kind="branch" aria-selected="true">Branches</button>
        <button type="button" role="tab" data-ref-kind="tag" aria-selected="false">Tags</button>
      </div>
      <input class="workspace-refsearch" type="search" aria-label="Search branches and tags" placeholder="Search branches…" autocomplete="off">
      <div class="workspace-refrows" role="listbox"></div>
    </div>
    <div class="error workspace-referror" role="status"></div>
  </div>`;
  const picker = host.querySelector(".workspace-refpicker");
  const trigger = picker.querySelector("[data-refpicker-toggle]");
  const triggerKind = picker.querySelector(".workspace-reftrigger-kind");
  const triggerName = picker.querySelector(".workspace-reftrigger-name");
  const menu = picker.querySelector(".workspace-refmenu");
  const search = picker.querySelector(".workspace-refsearch");
  const rowsHost = picker.querySelector(".workspace-refrows");
  const errorHost = picker.querySelector(".workspace-referror");
  let refs = [];
  let current = null;
  let activeKind = "branch";
  let disposed = false;
  let pending = false;
  let loadRequest = 0;
  let readRequest = 0;
  let cacheWrites = 0;
  const entityId = directoryCacheId(scope);
  const address = entityId ? cacheScope?.address({ entityId, kind: "refs" }) : null;

  const close = () => {
    menu.hidden = true;
    trigger.setAttribute("aria-expanded", "false");
  };
  const renderRows = () => {
    const query = search.value.trim().toLocaleLowerCase();
    const visible = refs.filter((ref) => {
      const searchable = `${ref.name} ${ref.remote || ""}`.toLocaleLowerCase();
      return ref.kind === activeKind && searchable.includes(query);
    });
    rowsHost.innerHTML = visible.length ? visible.map(rowHtml).join("") : `<div class="workspace-refempty">No ${kindLabel(activeKind).toLowerCase()} found</div>`;
  };
  const renderCurrent = () => {
    triggerKind.textContent = current?.kind === "tag" ? "Tag" : current?.kind === "detached" ? "Detached" : "Branch";
    triggerName.textContent = refLabel(current);
  };
  const selectKind = (kind) => {
    activeKind = kind;
    picker.querySelectorAll("[data-ref-kind]").forEach((tab) => tab.setAttribute("aria-selected", String(tab.dataset.refKind === kind)));
    search.placeholder = `Search ${kindLabel(kind).toLowerCase()}…`;
    renderRows();
  };
  const readRecord = async () => {
    if (!address) return;
    const version = ++readRequest;
    const record = await readCached(address);
    if (disposed || version !== readRequest) return;
    const listing = record?.value;
    if (!listing) return;
    refs = listing.refs || [];
    current = listing.current || refs.find((ref) => ref.current) || null;
    renderCurrent();
    renderRows();
    trigger.disabled = pending || refs.length === 0;
  };
  const unwatch = address ? subscribeCache(address, () => {
    cacheWrites += 1;
    void readRecord();
  }) : null;
  const load = async () => {
    const request = ++loadRequest;
    const before = cacheWrites;
    const answer = await callRpc("git.refs", scope);
    if (disposed || request !== loadRequest || pending) return;
    if (!address || cacheScope.active?.() === false || cacheWrites !== before) return;
    await writeCached(address, answer);
  };
  const checkout = async (fullRef) => {
    if (pending) return;
    pending = true;
    trigger.disabled = true;
    picker.classList.add("pending");
    errorHost.textContent = "";
    try {
      await callRpc("git.checkout_ref", { ...scope, full_ref: fullRef });
      if (disposed) return;
      close();
      await onCheckout?.();
    } catch (error) {
      if (!disposed) errorHost.textContent = error?.message || String(error);
    } finally {
      pending = false;
      if (!disposed) {
        trigger.disabled = refs.length === 0;
        picker.classList.remove("pending");
      }
    }
  };
  trigger.onclick = () => {
    const opening = menu.hidden;
    menu.hidden = !opening;
    trigger.setAttribute("aria-expanded", String(opening));
    if (opening) {
      search.value = "";
      renderRows();
      search.focus();
      load().catch((error) => { if (!disposed) errorHost.textContent = error?.message || String(error); });
    }
  };
  picker.querySelectorAll("[data-ref-kind]").forEach((tab) => { tab.onclick = () => selectKind(tab.dataset.refKind); });
  search.oninput = renderRows;
  rowsHost.onclick = (event) => {
    const row = event.target.closest("[data-ref]");
    if (row) void checkout(row.dataset.ref);
  };
  const outside = (event) => { if (!picker.contains(event.target)) close(); };
  const keydown = (event) => { if (event.key === "Escape") { close(); trigger.focus(); } };
  document.addEventListener("pointerdown", outside);
  picker.addEventListener("keydown", keydown);
  void readRecord();
  load().catch((error) => {
    if (disposed) return;
    if (!refs.length) triggerName.textContent = "Refs unavailable";
    errorHost.textContent = error?.message || String(error);
  });
  return { dispose() {
    disposed = true;
    unwatch?.();
    document.removeEventListener("pointerdown", outside);
    picker.removeEventListener("keydown", keydown);
  } };
}
