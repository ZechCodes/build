// The device fallback harness, held by the bridge. It is used only when a
// coding-agent creation request does not name a provider. Visible creation
// pickers send their displayed Claude Code or Codex provider and override it.
// The bridge's provider catalog supplies this selector's choices and labels.

import { providerOptionsHtml } from "./modelPicker.js";
import { deviceModelsAddress, deviceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";

export function defaultHarnessOf(settings, providers) {
  const defaultHarness = settings?.default_harness;
  if (typeof defaultHarness !== "string" || !providers.some((provider) => provider.id === defaultHarness)) {
    throw new Error("settings.default_harness is missing, malformed, or absent from models.list.providers");
  }
  return defaultHarness;
}

/** The panel, empty. `mountDefaultHarness` fills the select from the bridge —
 *  rendering it pre-filled would show a choice nobody has confirmed is the
 *  account's. */
export function defaultHarnessPanelHtml() {
  return `<div class="panel">
      <h3>🖥️ Fallback agent</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">This device fallback is used only when a coding-agent creation request does not name a provider. Creation pickers send the displayed Claude Code or Codex provider and override this fallback.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:180px"><label for="defaultharness">Fallback agent</label>
          <select id="defaultharness" disabled><option>loading…</option></select></div>
      </div>
      <div class="dim" id="harnesssaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="harnesserr"></div>
    </div>`;
}

/** Wire the panel to the bridge: paint from settings.get, save with
 *  settings.set, and repaint from whatever the bridge answers. The control
 *  shows what the account actually holds, never what was merely attempted. */
export async function mountDefaultHarness(host, { callRpc, deviceId = "", onSaved = async () => {} }) {
  const select = host.querySelector("#defaultharness");
  const saved = host.querySelector("#harnesssaved");
  const error = host.querySelector("#harnesserr");
  if (!select) return;

  let catalogProviders;
  let cachedSettings;
  const clearUnconfirmedSelection = () => {
    select.innerHTML = "";
    select.disabled = true;
  };
  const renderConfirmedSettings = () => {
    if (!catalogProviders || !cachedSettings) return;
    select.innerHTML = providerOptionsHtml(
      catalogProviders,
      defaultHarnessOf(cachedSettings, catalogProviders),
    );
    select.disabled = false;
    error.textContent = "";
  };
  const settingsRecord = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    cachedSettings = settings;
    try { renderConfirmedSettings(); } catch (failure) { clearUnconfirmedSelection(); error.textContent = failure.message; }
  }, { owner: select });
  const catalogRecord = watchSettingsRecord(deviceModelsAddress(deviceId), (catalog) => {
    if (!catalog) return;
    if (!Array.isArray(catalog.providers) || catalog.providers.length === 0) {
      clearUnconfirmedSelection();
      error.textContent = "models.list.providers is missing, malformed, or empty";
      return;
    }
    catalogProviders = catalog.providers;
    try { renderConfirmedSettings(); } catch (failure) { clearUnconfirmedSelection(); error.textContent = failure.message; }
  }, { owner: select });
  try {
    await Promise.all([
      settingsRecord.pull(() => callRpc("settings.get")),
      catalogRecord.pull(() => callRpc("models.list")),
    ]);
  } catch (e) {
    if (!cachedSettings || !catalogProviders) {
      clearUnconfirmedSelection();
      error.textContent = e.message;
    }
  }

  select.onchange = async () => {
    const chosen = select.value;
    select.disabled = true;
    error.textContent = "";
    saved.textContent = "Saving…";
    try {
      const settings = await callRpc("settings.set", { default_harness: chosen });
      defaultHarnessOf(settings, catalogProviders);
      await settingsRecord.write(settings);
      await onSaved(settings);
      saved.textContent = "Saved. This fallback applies when a coding-agent creation request does not name a provider.";
    } catch (saveError) {
      error.textContent = saveError.message;
      saved.textContent = "";
      await settingsRecord.read();
      try {
        await settingsRecord.pull(() => callRpc("settings.get"));
        await onSaved(cachedSettings);
        error.textContent = saveError.message;
      } catch (reloadError) {
        error.textContent = `${saveError.message}. Reload failed: ${reloadError.message}`;
      }
    }
    select.disabled = !catalogProviders || !cachedSettings;
  };
}
