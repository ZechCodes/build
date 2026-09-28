// Settings → Browser agent defaults: one model and reasoning effort per
// harness, and which harness a new task or agent starts on.
//
// The catalog is the creation device's (models.list), and the panel offers what
// every create surface offers: the two agents a person can create, never the
// carrier question behind either family — that one is the machine's, and is
// asked on its own page. Saved on every change: a preference with a Save button
// is one people forget to press.

import { esc } from "./text.js";
import { chosenProviderId } from "./agentChoice.js";
import { harnessDefaultsFor, loadHarnessDefaults, saveDefaultHarness, saveHarnessDefault } from "./agentDefaults.js";
import {
  creatableCatalog,
  effortOptionsHtml,
  effortSupported,
  modelInCatalog,
  modelNoteHtml,
  modelOptionsHtml,
  providerOptionsHtml,
} from "./modelPicker.js";

const FIELD_STYLE = "flex:1;min-width:150px";

const ACCOUNT_BLURB =
  "The model and reasoning effort each agent starts with, and which agent new work starts on. You can still change any of it per task, under the harness button in the New task sheet, or from the chat's model menu.";

/** The panel, empty: `mountHarnessDefaults` fills it from the catalog.
 *
 *  `prefix` names the element ids, because the same panel is mounted a second
 *  time by the workspace settings sheet (core/workspaceDefaults.js) against
 *  that workspace's own slot — two panels in one document need two sets of
 *  ids, or a `label for` points at the other panel's control. */
export function harnessDefaultsPanelHtml({
  prefix = "def",
  title = "🤖 Browser agent defaults",
  blurb = ACCOUNT_BLURB,
} = {}) {
  return `<div class="panel" data-harness-defaults="${esc(prefix)}">
      <h3>${esc(title)}</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">${esc(blurb)}</div>
      <div id="${esc(prefix)}harnesses"><span class="dim" style="font-size:13px">loading…</span></div>
      <div class="field" style="max-width:340px;margin-top:12px">
        <label for="${esc(prefix)}provider">Default agent</label>
        <select id="${esc(prefix)}provider"><option>loading…</option></select>
      </div>
      <div class="dim" id="${esc(prefix)}saved" style="font-size:12px;min-height:16px"></div>
    </div>`;
}

/** One harness's row: its model, and the reasoning levels that model takes. A
 *  stored model the catalog no longer carries is offered as itself rather than
 *  silently dropped. */
function harnessRowHtml(provider, preference, prefix) {
  const models = provider.models || [];
  const supported = effortSupported(models, preference.model);
  return `<div class="field-row" data-harness="${esc(provider.id)}" style="display:flex;gap:10px;flex-wrap:wrap">
      <div class="field" style="${FIELD_STYLE}">
        <label for="${esc(prefix)}model-${esc(provider.id)}">${esc(provider.label)} model</label>
        <select id="${esc(prefix)}model-${esc(provider.id)}" data-harness-model="${esc(provider.id)}">${modelOptionsHtml(models, preference.model, provider)}</select>
        ${modelNoteHtml(provider)}
      </div>
      <div class="field" style="${FIELD_STYLE}">
        <label for="${esc(prefix)}effort-${esc(provider.id)}">${esc(provider.label)} reasoning effort</label>
        <select id="${esc(prefix)}effort-${esc(provider.id)}" data-harness-effort="${esc(provider.id)}"${supported ? "" : " disabled"}>${effortOptionsHtml(
          provider.efforts || [],
          supported ? preference.effort : "",
          modelInCatalog(models, preference.model),
        )}</select>
      </div>
    </div>`;
}

/** Wire the panel to storage: paint from what is stored, save on every change,
 *  and repaint from what was stored — the controls show what the browser holds,
 *  never what was merely attempted. */
export function mountHarnessDefaults(host, { catalog, storage = localStorage, prefix = "def" }) {
  const rows = host.querySelector(`#${prefix}harnesses`);
  const providerSelect = host.querySelector(`#${prefix}provider`);
  const note = host.querySelector(`#${prefix}saved`);
  if (!rows || !providerSelect) return;
  const offered = creatableCatalog(catalog || {});

  const paint = () => {
    const defaults = loadHarnessDefaults(storage);
    rows.innerHTML = offered.providers
      .map((provider) => harnessRowHtml(provider, harnessDefaultsFor(defaults, provider.id), prefix))
      .join("");
    providerSelect.innerHTML = providerOptionsHtml(offered.providers, chosenProviderId(offered, defaults));
    wireRows();
  };
  const store = (write) => {
    write();
    paint();
    if (note) note.textContent = "Saved.";
  };
  const wireRows = () => {
    // A model belongs to its harness and an effort to its model, so a new model
    // drops the effort chosen under the old one.
    rows.querySelectorAll("[data-harness-model]").forEach((select) => {
      select.onchange = () =>
        store(() => saveHarnessDefault(select.dataset.harnessModel, { model: select.value, effort: "" }, storage));
    });
    rows.querySelectorAll("[data-harness-effort]").forEach((select) => {
      const providerId = select.dataset.harnessEffort;
      select.onchange = () =>
        store(() => {
          const preference = harnessDefaultsFor(loadHarnessDefaults(storage), providerId);
          saveHarnessDefault(providerId, { ...preference, effort: select.value }, storage);
        });
    });
  };

  providerSelect.onchange = () => store(() => saveDefaultHarness(providerSelect.value, storage));
  paint();
}
