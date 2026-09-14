// Review prioritization is a device choice held by the bridge. The control
// paints only a confirmed value: an older bridge that does not return the
// setting leaves it unavailable instead of silently turning the feature on.

export function triageEnabledOf(settings) {
  if (typeof settings?.triage_enabled !== "boolean") {
    throw new Error("Review prioritization is unavailable on this bridge. Update the bridge to enable it here.");
  }
  return settings.triage_enabled;
}

export function triageSettingPanelHtml() {
  return `<div class="panel" data-triage-setting>
      <h3>🔎 Diff triage</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">Run an extra read-only agent pass after changes are ready to order the diff for review. When off, diffs stay in file order.</div>
      <label><input type="checkbox" data-triage-setting="control" disabled> Prioritize changes for review</label>
      <div class="dim" data-triage-setting="saved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" data-triage-setting="error"></div>
    </div>`;
}

export async function mountTriageSetting(host, { callRpc }) {
  const panel = host.querySelector("[data-triage-setting]");
  if (!panel) return;
  const control = panel.querySelector('[data-triage-setting="control"]');
  const saved = panel.querySelector('[data-triage-setting="saved"]');
  const error = panel.querySelector('[data-triage-setting="error"]');

  let confirmed;
  const paint = (settings) => {
    confirmed = triageEnabledOf(settings);
    control.checked = confirmed;
  };

  try {
    paint(await callRpc("settings.get"));
    control.disabled = false;
  } catch (readError) {
    error.textContent = readError.message;
    return;
  }

  control.onchange = async () => {
    const chosen = control.checked;
    control.disabled = true;
    saved.textContent = "Saving…";
    error.textContent = "";
    try {
      paint(await callRpc("settings.set", { triage_enabled: chosen }));
      saved.textContent = confirmed
        ? "Saved. New and updated diffs will be prioritized for review."
        : "Saved. Diffs will stay in file order.";
    } catch (saveError) {
      saved.textContent = "";
      error.textContent = saveError.message;
      try {
        paint(await callRpc("settings.get"));
      } catch (reloadError) {
        control.checked = false;
        control.disabled = true;
        error.textContent = `${saveError.message}. Reload failed: ${reloadError.message}`;
        return;
      }
    }
    control.disabled = false;
  };
}
