import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { readCached, subscribeCache } from "../core/localCache.js";
import { readEnvironmentSupport, environmentSupportAddress } from "../core/environmentSupport.js";
import { environmentsAddress, admitEnvironmentListWrite, writeCachedEnvironments } from "../core/environmentCache.js";
import { watchSettingsRecord } from "../core/settingsRecords.js";
import { isolationOptionsHtml } from "../core/isolation.js";
import { requireEnvironmentAction, writeEnvironmentSetting } from "../core/environmentWrites.js";
import { onNavigationCommitted } from "../core/navigationLifetime.js";
import { watchSheetLifetime } from "../core/sheetLifetime.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { sourceEditing, sourceCardsHtml, mountSourceCards, readSourceEdits, restoreSourceEdits } from "./projectSources.js";
import { disposeRepoPickers } from "./repoPicker.js";
import { openBrowser } from "./browser.js";

const SOURCE_METHODS = {
  "project.update_source": "environment.update_source",
  "project.remove_source": "environment.remove_source",
  "project.sync_source": "environment.sync_source",
};
const asSourceOwner = environment => ({ ...environment, project_id: environment.environment_id, added_sources: environment.sources });

function generalHtml(environment) {
  return `<section class="ps-section"><h4>General</h4><div class="field"><label for="esname">Environment name</label>
    <input id="esname" ${fieldTraits("identifier")} value="${esc(environment.name)}"><button class="btn" id="esrename">Save name</button></div>
    <div class="field"><label for="eshome">Home path</label><input id="eshome" value="${esc(environment.home_path)}" readonly></div></section>`;
}

function sharedSources(host, environment, projects) {
  host.querySelectorAll(".ps-source").forEach(card => {
    const source = environment.sources[Number(card.dataset.sourceIndex)];
    const users = projects.filter(project => (project.sources || []).some(row => row.path === source.path)
      || project.environment_id === environment.environment_id).map(project => project.name);
    if (!users.length) return;
    const note = document.createElement("p"); note.className = "dim";
    note.textContent = users.map(name => `also used by ${name}`).join(" · ");
    card.append(note);
  });
}

export function openEnvironmentSettings(environmentId, { deviceId, callRpc }) {
  const sheet = $("#sheet"), scrim = $("#scrim");
  scrim.classList.add("show");
  sheet.innerHTML = settingsSheetHtml({ title: "Environment settings", bodyHtml: '<p class="dim">loading…</p>' });
  let frame = sheet.firstElementChild, painted = null, support = {}, projects = [], active = true;
  const current = () => active && sheet.firstElementChild === frame && scrim.classList.contains("show");
  const close = () => { const owns = current(); dispose(); if (owns) scrim.classList.remove("show"); };
  const record = watchSettingsRecord(environmentsAddress(deviceId), rows => {
    const environment = rows?.find(row => row.environment_id === environmentId);
    if (environment) paint(environment);
  });
  const unsubscribe = subscribeCache({ deviceId }, address => {
    if (["projects", "environment-settings-projects", environmentSupportAddress(deviceId).kind].includes(address.kind)) void readSupport();
  });
  const navigation = onNavigationCommitted(close);
  const dispose = watchSheetLifetime(current, () => {
    active = false; record.dispose(); unsubscribe(); navigation(); disposeRepoPickers(sheet);
  });
  async function readSupport() {
    const [actions, held] = await Promise.all([readEnvironmentSupport(deviceId), readCached({ deviceId, entityId: "", kind: "projects" })]);
    support = actions; projects = held?.value || [];
    await record.read();
  }
  const write = async (send, button, error) => {
    button.disabled = true; error.textContent = "";
    try {
      requireEnvironmentAction(deviceId, "rename");
      const changed = await send();
      await writeEnvironmentSetting(deviceId, changed); await record.read();
      return changed;
    } catch (failure) { error.textContent = failure.message; button.disabled = false; return null; }
  };
  function paint(environment) {
    if (!current()) return;
    const edits = painted ? readSourceEdits(sheet, asSourceOwner(painted)) : {};
    const name = sheet.querySelector("#esname");
    const draftName = name && name.value !== painted?.name ? name.value : environment.name;
    const owner = asSourceOwner(environment);
    const editing = support.rename ? sourceEditing(owner, true, {}) : { editable: () => false };
    disposeRepoPickers(sheet);
    sheet.innerHTML = settingsSheetHtml({ title: "Environment settings", bodyHtml: `${generalHtml(environment)}
      <section class="ps-section"><h4>Sources</h4>${sourceCardsHtml(owner, editing)}
      ${support.rename ? '<div class="row"><button class="btn" id="esfolder">Add folder…</button><button class="btn" id="esremote">Add Git remote…</button></div><div id="esadd"></div>' : ""}</section>
      <section class="ps-section"><h4>Isolation</h4><select id="esisolation" aria-label="Work isolation">${isolationOptionsHtml(environment.isolation, null, { inheritLabel: "Inherit device setting" })}</select></section>
      <div class="adderr" id="eserror" role="alert"></div><button class="btn" id="esclose">Close</button>` });
    frame = sheet.firstElementChild; painted = environment;
    sheet.querySelector("#esname").value = draftName;
    mountGeneral(environment);
    const sourceRpc = (method, params) => {
      requireEnvironmentAction(deviceId, "rename");
      const { project_id: _projectId, ...rest } = params;
      return callRpc(SOURCE_METHODS[method], { ...rest, environment_id: environmentId });
    };
    mountSourceCards(sheet, owner, { editing, callRpc: sourceRpc, deviceId, write, syncInFlight: new Set(), saveDraft: () => {}, onSaved: () => {} });
    restoreSourceEdits(sheet, owner, edits);
    sharedSources(sheet, environment, projects);
  }
  function mountGeneral(environment) {
    const error = sheet.querySelector("#eserror"), rename = sheet.querySelector("#esrename"), isolation = sheet.querySelector("#esisolation");
    rename.hidden = !support.rename; sheet.querySelector("#esname").readOnly = !support.rename; isolation.disabled = !support.rename;
    rename.onclick = () => void write(() => callRpc("environment.rename", { environment_id: environmentId, name: sheet.querySelector("#esname").value.trim() }), rename, error);
    isolation.onchange = () => void write(() => callRpc("environment.set_isolation", { environment_id: environmentId, isolation: isolation.value || null }), isolation, error);
    sheet.querySelector("#esclose").onclick = close;
    if (!support.rename) return;
    sheet.querySelector("#esremote").onclick = () => mountRemote(error, write);
    sheet.querySelector("#esfolder").onclick = () => browse(environment, error);
  }
  function mountRemote(error, writeSource) {
    const add = sheet.querySelector("#esadd");
    add.innerHTML = '<label>Clone url <input id="esurl"></label><label>Folder label <input id="eslabel"></label><button class="btn" id="esaddremote">Add remote</button>';
    const button = add.querySelector("button");
    button.onclick = () => void writeSource(() => callRpc("environment.add_source", { environment_id: environmentId,
      remote: add.querySelector("#esurl").value.trim(), ...(add.querySelector("#eslabel").value.trim() ? { name: add.querySelector("#eslabel").value.trim() } : {}) }), button, error);
  }
  async function browse(environment, error) {
    const container = sheet.querySelector("#esadd");
    try {
      await openBrowser({ container, deviceId, callRpc, title: "Add folder", gitOnly: false, startPath: environment.home_path,
        onCancel: () => { container.replaceChildren(); },
        onChoose: path => void write(() => callRpc("environment.add_source", { environment_id: environmentId, path }), sheet.querySelector("#esfolder"), error),
      });
    } catch (failure) { error.textContent = failure.message; }
  }
  void readSupport();
  void (async () => {
    const admission = await admitEnvironmentListWrite(deviceId, current);
    const answer = await callRpc("environment.list");
    await writeCachedEnvironments(deviceId, answer.environments, admission, current);
  })().catch(error => { if (current() && !painted) sheet.querySelector(".dim").textContent = error.message; });
  return { dispose, whenCachePainted: record.whenPainted };
}
