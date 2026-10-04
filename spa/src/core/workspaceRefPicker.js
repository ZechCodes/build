import { appBehindMark, bridgeBehindMark, esc } from "./text.js";
import { refLabel } from "./workspaceModel.js";
import { directoryCacheId } from "./directoryScope.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";
import { fieldTraits } from "./fieldTraits.js";
import { contextFor, onDeviceStateChanged, whenGreeted } from "./deviceContexts.js";
import { isTransientTransportError } from "./transientRead.js";

const kindLabel = (kind) => kind === "tag" ? "Tags" : "Branches";

/** A read refused because the machine cannot be asked right now, or died on the
 *  wire, rather than one the bridge answered no. That is the device strip's
 *  news, not the picker's: the cached refs stay on screen as they are, and the
 *  next session reads again. */
const BEHIND = new Set([appBehindMark, bridgeBehindMark]);
const errorText = (error) => error?.message || String(error);
const machineAway = (error) => isTransientTransportError(error) || BEHIND.has(errorText(error).trim());

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
      <input class="workspace-refsearch mini" type="search" aria-label="Search branches and tags" placeholder="Search branches…" ${fieldTraits("search")}>
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
  let visible = true;
  let pending = false;
  let loadWhenShown = false;
  let loadRequest = 0;
  let readRequest = 0;
  let cacheWrites = 0;
  let readFailed = false; // the last refs read did not land; the next session reads again
  let readShown = false; // the status holds a read's refusal, not a checkout's
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
  const paintListing = (listing) => {
    refs = listing.refs || [];
    current = listing.current || refs.find((ref) => ref.current) || null;
    renderCurrent();
    renderRows();
    trigger.disabled = pending || refs.length === 0;
  };
  const clearRead = () => {
    if (!readShown) return;
    readShown = false;
    errorHost.textContent = "";
    if (!refs.length) triggerName.textContent = "Loading refs…";
  };
  const readLanded = () => {
    readFailed = false;
    clearRead();
  };
  const readRefused = (error) => {
    readFailed = true;
    if (machineAway(error)) return;
    if (!refs.length) triggerName.textContent = "Refs unavailable";
    errorHost.textContent = errorText(error);
    readShown = true;
  };
  const readRecord = async (written = false) => {
    if (!address) return false;
    const version = ++readRequest;
    const record = await readCached(address);
    if (disposed || version !== readRequest) return true;
    const listing = record?.value;
    if (!listing) return false;
    // A record written while mounted is a read that landed, whoever made it.
    if (written) readLanded();
    paintListing(listing);
    return !listing.stale;
  };
  const unwatch = address ? subscribeCache(address, () => {
    cacheWrites += 1;
    void readRecord(true).then((held) => {
      if (!disposed && !held) void load();
    });
  }) : null;
  const mayWrite = (before) => !pending && Boolean(address) && cacheScope.active?.() !== false && cacheWrites === before;
  const load = async () => {
    if (!visible) {
      loadWhenShown = true;
      return;
    }
    loadWhenShown = false;
    const request = ++loadRequest;
    const before = cacheWrites;
    try {
      const answer = await callRpc("git.refs", scope);
      if (disposed || request !== loadRequest) return;
      readLanded();
      if (mayWrite(before)) await writeCached(address, answer);
    } catch (error) {
      if (!disposed && request === loadRequest) readRefused(error);
    }
  };
  // A session adopted for this machine is the state change that retires what
  // the last one failed to read: the diagnostic goes, and the new session is
  // asked again once its greeting says it can be.
  const deviceId = cacheScope?.deviceId || null;
  let session = contextFor(deviceId)?.session || null;
  const unwatchDevice = deviceId ? onDeviceStateChanged(() => {
    const context = contextFor(deviceId);
    const next = context?.session || null;
    if (disposed || !next || next === session) return;
    session = next;
    if (!readFailed) return;
    clearRead();
    void whenGreeted(context, () => load());
  }) : null;
  const checkout = async (fullRef) => {
    if (pending || !visible) return;
    pending = true;
    trigger.disabled = true;
    picker.classList.add("pending");
    errorHost.textContent = "";
    readShown = false;
    try {
      await callRpc("git.checkout_ref", { ...scope, full_ref: fullRef });
      if (disposed) return;
      close();
      await onCheckout?.();
    } catch (error) {
      if (!disposed) errorHost.textContent = errorText(error);
      return;
    } finally {
      pending = false;
      if (!disposed) {
        trigger.disabled = refs.length === 0;
        picker.classList.remove("pending");
      }
    }
    if (!disposed) await load();
  };
  trigger.onclick = () => {
    if (!visible) return;
    const opening = menu.hidden;
    menu.hidden = !opening;
    trigger.setAttribute("aria-expanded", String(opening));
    if (opening) {
      search.value = "";
      renderRows();
      search.focus();
      void load();
    }
  };
  picker.querySelectorAll("[data-ref-kind]").forEach((tab) => { tab.onclick = () => selectKind(tab.dataset.refKind); });
  search.oninput = renderRows;
  rowsHost.onclick = (event) => {
    const row = event.target.closest("[data-ref]");
    if (row) void checkout(row.dataset.ref);
  };
  const outside = (event) => { if (!picker.contains(event.target)) close(); };
  const keydown = (event) => { if (visible && event.key === "Escape") { close(); trigger.focus(); } };
  document.addEventListener("pointerdown", outside);
  picker.addEventListener("keydown", keydown);
  void readRecord().then((held) => {
    if (!disposed && !held) void load();
  });
  return { setVisible(shown) {
    if (disposed || shown === visible) return;
    visible = shown;
    if (!shown) {
      close();
      if (picker.contains(document.activeElement)) document.activeElement.blur();
      document.removeEventListener("pointerdown", outside);
    } else {
      document.addEventListener("pointerdown", outside);
      if (loadWhenShown) void load();
    }
  }, dispose() {
    disposed = true;
    unwatch?.();
    unwatchDevice?.();
    document.removeEventListener("pointerdown", outside);
    picker.removeEventListener("keydown", keydown);
  } };
}
