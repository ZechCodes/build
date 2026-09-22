// Whether an issue an agent files is one the user hears about (#65).
//
// The one switch watching needs. It is the DEVICE's, beside the other bridge
// settings, because the bridge is what decides whether to watch at the moment
// an agent files an issue — the browser is not there, and two browsers holding
// different answers would make "did I ask to see this?" depend on which one
// was open.
//
// Default ON. An agent filing an issue for you is the case the feature exists
// for — "if they ask the agent to create an issue to workshop something the
// user sees it in their inbox" — so it works before anybody visits Settings,
// and the switch is how someone who does not want it says so.
//
// Painted from settings.get, saved with settings.set, repainted from whatever
// the bridge answers: the control shows what the machine actually holds, never
// what was merely attempted.

import { deviceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";

/**
 * Whether this machine watches what its agents file, as a settings payload
 * states it.
 *
 * Absent means on, which is the same thing the bridge means by leaving it out.
 * Never throws: a machine answering something this client cannot read is not a
 * reason to leave the reader with no answer.
 */
export const watchesAgentFiledIssues = (settings) => settings?.watch_agent_filed_issues !== false;

/** The panel, with the switch disabled until the machine has said. Rendering
 *  it pre-checked would show a choice nobody has confirmed is this machine's. */
export function watchSettingPanelHtml() {
  return `<div class="panel" data-watch-setting>
      <h3>👀 Watching</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">Issues you file, comment on or are assigned always reach your inbox. This is about the ones an agent files: leave it on and you see those too, which is what you want when you have asked an agent to open something for you.</div>
      <label class="field-row" style="display:flex;gap:8px;align-items:center">
        <input type="checkbox" id="watchagentissues" disabled />
        <span>Watch issues agents file for me</span>
      </label>
      <div class="dim" id="watchagentissuessaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="watchagentissueserr"></div>
    </div>`;
}

/**
 * Wire the panel to one machine's caller.
 *
 * Mounting is what reads, so a page that reconnects mounts again: a switch
 * painted over a connection that has gone shows what WAS there.
 */
export async function mountWatchSetting(host, { callRpc, deviceId = "", onSaved } = {}) {
  const panel = host.querySelector("[data-watch-setting]");
  if (!panel) return;
  const box = panel.querySelector("#watchagentissues");
  const saved = panel.querySelector("#watchagentissuessaved");
  const failed = panel.querySelector("#watchagentissueserr");

  const show = (settings) => {
    box.checked = watchesAgentFiledIssues(settings);
    box.disabled = false;
    failed.textContent = "";
  };
  let painted = false;
  const record = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    if (!settings) return;
    show(settings);
    painted = true;
  }, { owner: panel });

  box.onchange = async () => {
    const wanted = box.checked;
    saved.textContent = "Saving…";
    failed.textContent = "";
    try {
      // Repainted from the ANSWER rather than from `wanted`: the machine is
      // what holds this, and what it says it holds is what the switch shows.
      await record.write(await callRpc("settings.set", { watch_agent_filed_issues: wanted }));
      saved.textContent = "Saved";
      onSaved?.();
    } catch (error) {
      // The switch goes back: it never shows a choice the machine refused.
      await record.read();
      saved.textContent = "";
      failed.textContent = error.message || String(error);
    }
  };

  try {
    await record.pull(() => callRpc("settings.get"));
  } catch (error) {
    // A machine that cannot be read offers no switch — a control that looks
    // settable but cannot be saved is worse than one that says why.
    if (!painted) box.disabled = true;
    failed.textContent = error.message || String(error);
  }
}
