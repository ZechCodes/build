// The existing console over all original sources. A source choice belongs to
// the next shell; each standing tab keeps its captured location for life.
import { el } from "../dom.js";
import { esc } from "./text.js";
import { consoleKey, consoleSize, consoleTakesKey, grownConsoleSize, toggledConsoleSize } from "./consoleModel.js";
import { consoleHeadHtml, consoleToggleHtml, consoleTabHtml, consoleNewTerminalHtml, consoleGrowHtml, mountUserTerminalPane } from "./console.js";
import { terminalManager } from "../terminal/manager.js";
import { directoryCacheId } from "./directoryScope.js";
import { readCached, mergeCachedTogether, subscribeCache } from "./localCache.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { hasProjectTerminalSource } from "./projectTerminalCache.js";
import { readProjectTerminalSupport } from "./projectTerminalSupport.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { notifyError } from "./notify.js";
import { ICON_FOLDER } from "./icons.js";

const scopeOf = (projectId, source) => ({ project_id: projectId, source_id: source.id, source_path: source.path });
const tabKey = (scope, termId) => JSON.stringify([directoryCacheId(scope), termId]);
const sourcesOf = (project) => project?.sources || (project?.path ? [{ id: "primary", name: project.name, path: project.path }] : []);
const labelOf = (source) => source.name || source.mount || source.id;

export function mountProjectConsole(host, context) {
  const manager = terminalManager();
  const key = consoleKey(context);
  const address = (scope) => ({ deviceId: context.deviceId, entityId: directoryCacheId(scope), kind: "terminals" });
  const projectsAddress = { deviceId: context.deviceId, entityId: "", kind: "projects" };
  const state = { disposed: false, sources: [], tabs: [], selected: null, sourceId: null, supported: false,
    size: "collapsed", reopen: "half", pane: null, paneKey: null, mountAbort: null, mounting: 0, listeners: new Set(), records: new Map() };
  let reading = null;
  let again = false;
  let uiReady = false;
  let ui;
  const active = () => !state.disposed && (!context.cacheScope || context.cacheScope.active());
  const selections = watchUiState(uiAddress({ deviceId: context.deviceId, entityId: key, view: "console", kind: "selection" }), (saved) => {
    state.sourceId = saved?.sourceId || state.sourceId;
    state.selected = saved?.selected || state.selected;
    if (uiReady) paint();
  });
  const remember = () => selections.write({ sourceId: state.sourceId, selected: state.selected });
  const emitSize = () => { for (const listener of state.listeners) listener(state.size); };
  const setSize = (size) => ui.write({ size, reopenSize: size === "collapsed" ? state.reopen : size });
  const toggle = () => setSize(toggledConsoleSize(state.size, state.reopen));
  const currentTab = () => state.tabs.find((tab) => tab.id === state.selected);
  const currentSource = () => state.sources.find((source) => source.id === state.sourceId);

  function disposePane() {
    state.mounting += 1;
    state.mountAbort?.abort();
    state.pane?.dispose();
    if (state.paneKey) manager.detach(state.paneKey.termId, state.mountAbort.signal);
    state.pane = null; state.paneKey = null; state.mountAbort = null;
  }

  async function mountPane(region, tab) {
    disposePane();
    state.paneKey = tab;
    state.mountAbort = new AbortController();
    const generation = state.mounting;
    region.innerHTML = '<div class="termpane console-pane"></div>';
    const target = region.firstElementChild;
    try {
      const pane = await mountUserTerminalPane(target, tab.termId, { scope: tab.scope, deviceId: context.deviceId, signal: state.mountAbort.signal, recover: true,
        stands: () => active() && generation === state.mounting, onExit: () => void remove(tab) });
      if (state.disposed || generation !== state.mounting) { pane.dispose(); return; }
      state.pane = pane;
    } catch (error) {
      if (state.disposed || generation !== state.mounting) return;
      if (/unknown term_id/.test(error.message || "")) void remove(tab);
      else target.innerHTML = `<div class="empty selectable">${esc(error.message || "Terminal unavailable")}</div>`;
    }
  }

  function paintBody() {
    const body = host.querySelector(".console-body");
    body.hidden = state.size === "collapsed";
    if (body.hidden) { disposePane(); body.innerHTML = ""; return; }
    const tab = currentTab();
    if (state.supported && tab) {
      if (state.paneKey?.id !== tab.id) void mountPane(body, tab);
      return;
    }
    disposePane();
    const message = state.supported ? "Choose a source and press + to open a terminal." : "Update this device to use project Terminal.";
    body.innerHTML = `<div class="console-empty"><span class="dim">${esc(message)}</span></div>`;
  }

  function paintPicker() {
    const picker = host.querySelector("[aria-label='Terminal source']");
    const focused = document.activeElement === picker;
    const options = state.sources.map((source) => `<option value="${esc(source.id)}">${esc(labelOf(source))}</option>`).join("");
    if (picker.innerHTML !== options) picker.innerHTML = options;
    picker.value = state.sourceId || "";
    picker.disabled = !state.supported || !state.sources.length;
    if (focused) picker.focus();
  }

  function wireTab(node) {
    const find = () => state.tabs.find((tab) => tab.id === node.dataset.key);
    node.querySelector(".console-tab-name").onclick = () => {
      state.selected = node.dataset.key; void remember();
      if (state.size === "collapsed") void setSize(state.reopen); else paint();
    };
    node.querySelector("[data-close]").onclick = () => void close(find());
  }

  function placeNewTerminal(strip, add) {
    if (strip.lastElementChild === add) return;
    const focused = document.activeElement === add;
    if (add.parentNode === strip && typeof strip.moveBefore === "function") strip.moveBefore(add, null);
    else strip.append(add);
    if (focused) add.focus();
  }

  function paint() {
    if (state.disposed || !uiReady) return;
    if (!host.querySelector(".console")) {
      host.innerHTML = `<div class="console"><div class="console-head">${consoleHeadHtml(state.size)}</div><div class="console-body" hidden></div></div>`;
      host.querySelector("#console-toggle").onclick = toggle;
      const controls = host.querySelector(".console-controls");
      controls.innerHTML = `<span class="console-source-control"><select class="console-source mini" aria-label="Terminal source"></select><span class="console-source-icon" aria-hidden="true">${ICON_FOLDER}</span></span>` + consoleGrowHtml(state.size);
      controls.querySelector("select").onchange = (event) => { state.sourceId = event.target.value; void remember(); };
      controls.querySelector(".console-grow").onclick = () => void setSize(grownConsoleSize(state.size));
    }
    host.dataset.size = state.size;
    patchElement(host.querySelector("#console-toggle"), el(consoleToggleHtml(state.size)));
    paintPicker();
    const strip = host.querySelector(".console-tabs");
    patchList(strip, state.tabs, { keyOf: (tab) => tab.id, render: (tab) => consoleTabHtml(tab, state.selected), wire: wireTab });
    let add = strip.querySelector(".console-new");
    if (!add) { add = el(consoleNewTerminalHtml()); add.onclick = () => void create(); strip.append(add); }
    add.disabled = !state.supported || !currentSource() || !state.records.get(currentSource()?.id);
    placeNewTerminal(strip, add);
    const grow = host.querySelector(".console-grow");
    patchElement(grow, el(consoleGrowHtml(state.size))); grow.hidden = state.size === "collapsed";
    paintBody(); emitSize();
  }

  function reconcileSelections(sources) {
    if (!currentSource()) state.sourceId = sources.find((source) => source.id === context.sourceId)?.id || sources[0]?.id || null;
    if (!currentTab()) state.selected = state.tabs[0]?.id || null;
  }

  function listMissing(records) {
    if (!state.supported) return;
    for (const { source, record } of records) if (!record) void list(source);
  }

  async function collect() {
    const project = ((await readCached(projectsAddress))?.value || []).find((row) => (row.project_id || row.id) === context.projectId);
    const sources = sourcesOf(project);
    const support = await readProjectTerminalSupport(context.deviceId);
    const records = await Promise.all(sources.map(async (source) => ({ source, record: await readCached(address(scopeOf(context.projectId, source))) })));
    if (!active()) return;
    state.sources = sources;
    state.supported = support.projectSources;
    state.records = new Map(records.map(({ source, record }) => [source.id, record]));
    state.tabs = records.flatMap(({ source, record }) => (record?.value?.tabs || []).filter((tab) => tab?.term_id).map((tab, index) => {
      const scope = scopeOf(context.projectId, source);
      return { id: tabKey(scope, tab.term_id), termId: tab.term_id, scope, label: `${labelOf(source)} · ${tab.name || `Terminal ${index + 1}`}` };
    }));
    reconcileSelections(sources);
    paint();
    listMissing(records);
  }

  const pending = new Set();
  async function list(source) {
    const scope = scopeOf(context.projectId, source);
    const at = address(scope);
    if (pending.has(at.entityId)) return;
    pending.add(at.entityId);
    try {
      const tabs = await manager.listTerminals(scope);
      if (!state.disposed && state.sources.some((item) => item.id === source.id && item.path === source.path)) {
        await mutateRoster(scope, (held) => held ? null : { tabs });
      }
    } catch { /* cached content stands until a later cache update retries the missing roster */ }
    finally { pending.delete(at.entityId); }
  }

  function read() {
    again = true;
    if (reading) return reading;
    reading = (async () => { do { again = false; await collect(); } while (again && !state.disposed); })().finally(() => { reading = null; });
    return reading;
  }

  async function mutateRoster(scope, mutate) {
    return mergeCachedTogether([projectsAddress, address(scope)], ([projects, held]) => {
      if (!active() || !hasProjectTerminalSource(projects, scope)) return null;
      return [null, mutate(held)];
    });
  }
  async function change(tab, mutate) {
    await mutateRoster(tab.scope, (held) => ({ tabs: mutate(held?.tabs || []) }));
  }
  const remove = (tab) => change(tab, (tabs) => tabs.filter((entry) => entry.term_id !== tab.termId));
  async function close(tab) {
    if (!tab) return;
    try { await manager.closeTerminal(tab.termId, tab.scope); await remove(tab); }
    catch (error) { notifyError("Could not close the terminal", error.message || "error"); }
  }
  async function create() {
    const source = currentSource();
    if (!state.supported || !source) return;
    const scope = scopeOf(context.projectId, source);
    try {
      const created = await manager.createTerminal(scope, 80, 24);
      if (state.disposed || !state.sources.some((item) => item.id === source.id && item.path === source.path) || !created?.term_id) return;
      state.selected = tabKey(scope, created.term_id);
      await change({ scope }, (tabs) => tabs.some((tab) => tab.term_id === created.term_id) ? tabs : [...tabs, created]);
      await read();
      state.selected = tabKey(scope, created.term_id);
      await remember();
      await setSize(state.reopen);
    } catch (error) { notifyError("Could not open a terminal", error.message || "error"); }
  }
  function onKey(event) {
    if (event.key !== "`" || event.metaKey || event.ctrlKey || event.altKey || !consoleTakesKey(event.target)) return;
    event.preventDefault(); void toggle();
  }
  const unwatch = subscribeCache({ deviceId: context.deviceId }, () => void read());
  document.addEventListener("keydown", onKey);
  ui = watchUiState(uiAddress({ deviceId: context.deviceId, entityId: key, view: "console", kind: "fold" }), (saved) => {
    state.size = consoleSize(saved?.size); state.reopen = saved?.reopenSize === "full" ? "full" : "half";
    if (uiReady) paint();
  });
  void Promise.all([ui.ready, selections.ready]).then(() => { uiReady = true; void read(); });
  return {
    toggle, size: () => state.size,
    subscribeSize(listener) { state.listeners.add(listener); listener(state.size); return () => state.listeners.delete(listener); },
    dispose() { state.disposed = true; unwatch(); ui.dispose(); selections.dispose(); document.removeEventListener("keydown", onKey); disposePane(); state.listeners.clear(); host.innerHTML = ""; delete host.dataset.size; },
  };
}
