// How each agent family is presented after it starts. The bridge owns these
// preferences; this panel only paints confirmed values and sends one family at
// a time so changing Codex cannot overwrite Claude Code (or vice versa).

import { deviceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";

export const AGENT_MODE_FAMILIES = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];

export const AGENT_MODES = [
  { id: "headless", label: "Headless" },
  { id: "tui", label: "TUI" },
];

const modeIds = new Set(AGENT_MODES.map(({ id }) => id));

function confirmedMode(settings, family) {
  const modes = settings?.agent_modes;
  if (!modes || typeof modes !== "object" || Array.isArray(modes)) {
    throw new Error("Agent modes are unavailable on this bridge. Update the bridge to choose Headless or TUI here.");
  }
  const mode = modes[family];
  if (!modeIds.has(mode)) {
    throw new Error(`settings.agent_modes.${family} is missing or malformed`);
  }
  return mode;
}

function optionsHtml(selected) {
  return AGENT_MODES.map(({ id, label }) =>
    `<option value="${id}"${id === selected ? " selected" : ""}>${label}</option>`,
  ).join("");
}

export function agentModesPanelHtml() {
  const fields = AGENT_MODE_FAMILIES.map(({ id, label }) => `
        <div class="field" style="flex:1;min-width:180px"><label for="agentmode-${id}">${label}</label>
          <select id="agentmode-${id}" data-agent-mode="${id}" disabled><option>loading…</option></select>
          <div class="dim" data-agent-mode-status="${id}" style="font-size:12px;min-height:16px"></div>
        </div>`).join("");
  return `<div class="panel" data-agent-modes-panel>
      <h3>🤖 Agent modes</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">Choose whether each agent runs through its conversation interface or its terminal UI. Applies to new agents; existing agents keep their mode.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">${fields}
      </div>
      <div class="adderr" data-agent-modes-error></div>
    </div>`;
}

export async function mountAgentModes(host, { callRpc, deviceId = "", onSaved }) {
  const panel = host.querySelector("[data-agent-modes-panel]");
  if (!panel) return;
  const error = panel.querySelector("[data-agent-modes-error]");
  const controls = new Map(AGENT_MODE_FAMILIES.map(({ id }) => [
    id,
    {
      select: panel.querySelector(`[data-agent-mode="${id}"]`),
      status: panel.querySelector(`[data-agent-mode-status="${id}"]`),
      confirmed: null,
    },
  ]));

  const disableAll = () => {
    controls.forEach(({ select, status }) => {
      select.innerHTML = '<option value="">Unavailable</option>';
      select.disabled = true;
      status.textContent = "";
    });
  };

  const paintConfirmedSettings = (settings) => {
    const confirmed = new Map(AGENT_MODE_FAMILIES.map(({ id }) => [id, confirmedMode(settings, id)]));
    controls.forEach((control, family) => {
      control.confirmed = confirmed.get(family);
      control.select.innerHTML = optionsHtml(control.confirmed);
      control.select.disabled = false;
    });
  };

  let painted = false;
  const record = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    if (!settings) return;
    try {
      paintConfirmedSettings(settings);
      painted = true;
      error.textContent = "";
    } catch (readError) {
      disableAll();
      error.textContent = readError.message;
    }
  }, { owner: panel });
  try {
    await record.pull(() => callRpc("settings.get"));
  } catch (readError) {
    if (!painted) {
      disableAll();
      error.textContent = readError.message;
    }
  }

  controls.forEach((control, family) => {
    control.select.onchange = async () => {
      const chosen = control.select.value;
      control.select.disabled = true;
      control.status.textContent = "Saving…";
      error.textContent = "";
      try {
        const settings = await callRpc("settings.set", { agent_modes: { [family]: chosen } });
        confirmedMode(settings, family);
        await record.write(settings);
        control.status.textContent = "Saved.";
        await onSaved?.(settings);
      } catch (saveError) {
        control.status.textContent = "";
        await record.read();
        try {
          await record.pull(() => callRpc("settings.get"));
          await onSaved?.();
          error.textContent = saveError.message;
        } catch (reloadError) {
          error.textContent = `${saveError.message}. Reload failed: ${reloadError.message}`;
        }
      }
      control.select.disabled = !painted;
    };
  });
}
