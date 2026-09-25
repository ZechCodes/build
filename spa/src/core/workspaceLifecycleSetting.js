// When a workspace counts as quiet, and whether a quiet one loses its build
// output (#167).
//
// Both are the DEVICE's, beside the other bridge settings: the bridge's reclaim
// service acts on them every hour whether or not a browser is open. An
// environment variable on the machine (`BRIDGE_WORKSPACE_IDLE_SECS`,
// `BRIDGE_WORKSPACE_PRUNE`) still wins; the bridge names the setting it pins,
// and the panel holds that control still and says which variable holds it.
//
// Painted from the cached settings record, saved with settings.set, repainted
// from whatever the bridge answers: the controls show what the machine holds,
// never what was merely attempted. A bridge older than 1.25.0 answers without
// the fields, and the panel says so in a sentence instead of offering controls
// it cannot save.

import { deviceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";

const PINNED_BY = Object.freeze({
  workspace_idle_secs: "BRIDGE_WORKSPACE_IDLE_SECS",
  workspace_prune: "BRIDGE_WORKSPACE_PRUNE",
});

/** The units the threshold is read in, largest first, and their seconds. */
const UNITS = Object.freeze([["hours", 3600], ["minutes", 60], ["seconds", 1]]);
const UNIT_SECONDS = Object.freeze(Object.fromEntries(UNITS));
const SINGULAR = Object.freeze({ hours: "hour", minutes: "minute", seconds: "second" });

/** The saved seconds in the largest unit that holds them exactly, so what the
 *  panel shows is always what the bridge holds (review 1): 86400 is 24 hours,
 *  5400 is 90 minutes, 17 is 17 seconds. */
function idleReading(seconds) {
  const [unit, size] = UNITS.find(([, unitSeconds]) => seconds % unitSeconds === 0);
  return { amount: seconds / size, unit };
}

/**
 * What one settings answer says about the workspace lifecycle. `known` is
 * false before any answer and on a bridge that predates the settings.
 */
export function workspaceLifecycleOf(settings) {
  const known = Number.isInteger(settings?.workspace_idle_secs) && settings.workspace_idle_secs > 0;
  const pinned = new Set(Array.isArray(settings?.workspace_pinned) ? settings.workspace_pinned : []);
  return {
    known,
    idle: known ? idleReading(settings.workspace_idle_secs) : null,
    prune: settings?.workspace_prune === true,
    idlePinned: pinned.has("workspace_idle_secs"),
    prunePinned: pinned.has("workspace_prune"),
  };
}

const pinnedSentence = (pinned, setting) => (pinned ? `${PINNED_BY[setting]} sets this on this machine.` : "");

/** The panel, with both controls disabled until the machine, or what this
 *  device last cached of it, has said. */
export function workspaceLifecyclePanelHtml() {
  return `<div class="panel" data-workspace-lifecycle-setting>
      <h3>🧹 Quiet workspaces</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">A workspace with no agent turn, commit or file change for this long is quiet. The project agent is told, and merges it, deletes it or asks you.</div>
      <div data-lifecycle-controls>
        <label class="field-row" style="display:flex;gap:8px;align-items:center">
          <span>Quiet after</span>
          <input type="number" id="workspaceidlehours" min="0" step="any" inputmode="decimal" autocomplete="off" autocapitalize="off" style="width:6em" disabled />
          <span id="workspaceidleunit">hours</span>
        </label>
        <div class="dim" id="workspaceidlepinned" style="font-size:12px"></div>
        <label class="field-row" style="display:flex;gap:8px;align-items:center;margin-top:8px">
          <input type="checkbox" id="workspaceprune" disabled />
          <span>Drop build output from quiet workspaces</span>
        </label>
        <div class="dim" style="font-size:12px">node_modules, target, .venv and dist, from a quiet workspace whose work is safe somewhere else. The source stays.</div>
        <div class="dim" id="workspaceprunepinned" style="font-size:12px"></div>
      </div>
      <div class="dim" id="workspacelifecycleolder" style="font-size:13px"></div>
      <div class="dim" id="workspacelifecyclesaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="workspacelifecycleerr"></div>
    </div>`;
}

/** The panel's elements, found once. */
function panelParts(panel) {
  const find = (selector) => panel.querySelector(selector);
  return {
    controls: find("[data-lifecycle-controls]"),
    hours: find("#workspaceidlehours"),
    unit: find("#workspaceidleunit"),
    prune: find("#workspaceprune"),
    idlePinned: find("#workspaceidlepinned"),
    prunePinned: find("#workspaceprunepinned"),
    older: find("#workspacelifecycleolder"),
    saved: find("#workspacelifecyclesaved"),
    failed: find("#workspacelifecycleerr"),
  };
}

/** Paint one settings answer onto the panel. */
function paint(parts, settings) {
  const lifecycle = workspaceLifecycleOf(settings);
  parts.controls.hidden = !lifecycle.known;
  parts.older.textContent = lifecycle.known
    ? ""
    : "This machine's bridge is older than these settings. Update it to change them here.";
  if (!lifecycle.known) return;
  parts.hours.value = String(lifecycle.idle.amount);
  parts.hours.dataset.unit = lifecycle.idle.unit;
  parts.unit.textContent = lifecycle.idle.amount === 1 ? SINGULAR[lifecycle.idle.unit] : lifecycle.idle.unit;
  parts.hours.disabled = lifecycle.idlePinned;
  parts.idlePinned.textContent = pinnedSentence(lifecycle.idlePinned, "workspace_idle_secs");
  parts.prune.checked = lifecycle.prune;
  parts.prune.disabled = lifecycle.prunePinned;
  parts.prunePinned.textContent = pinnedSentence(lifecycle.prunePinned, "workspace_prune");
}

/** The threshold the field asks for, in the unit it is showing, as the whole
 *  seconds the bridge holds, or null when that is not above zero. */
function idleSecondsOf(value, unit) {
  const seconds = Math.round(Number(value) * (UNIT_SECONDS[unit] || UNIT_SECONDS.hours));
  return Number.isFinite(seconds) && seconds > 0 ? seconds : null;
}

/**
 * Wire the panel to one machine's caller.
 *
 * Mounting is what reads, so a page that reconnects mounts again. What this
 * device cached of the machine's settings paints first.
 */
export async function mountWorkspaceLifecycleSetting(host, { callRpc, deviceId = "", onSaved } = {}) {
  const panel = host.querySelector("[data-workspace-lifecycle-setting]");
  if (!panel) return;
  const parts = panelParts(panel);
  let painted = false;
  const record = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    if (!settings) return;
    paint(parts, settings);
    painted = true;
  }, { owner: panel });

  const save = async (patch) => {
    parts.saved.textContent = "Saving…";
    parts.failed.textContent = "";
    try {
      // Repainted from the ANSWER: what the machine says it holds, pins
      // included, is what the controls show.
      await record.write(await callRpc("settings.set", patch));
      parts.saved.textContent = "Saved";
      onSaved?.();
    } catch (error) {
      await record.read();
      parts.saved.textContent = "";
      parts.failed.textContent = error.message || String(error);
    }
  };

  parts.hours.onchange = async () => {
    const seconds = idleSecondsOf(parts.hours.value, parts.hours.dataset.unit);
    if (seconds === null) {
      await record.read();
      parts.failed.textContent = "Enter a number above zero.";
      return;
    }
    await save({ workspace_idle_secs: seconds });
  };
  parts.prune.onchange = () => save({ workspace_prune: parts.prune.checked });

  try {
    await record.pull(() => callRpc("settings.get"));
  } catch (error) {
    // A machine that cannot be read and was never cached offers no control.
    if (!painted) {
      parts.hours.disabled = true;
      parts.prune.disabled = true;
    }
    parts.failed.textContent = error.message || String(error);
  }
}
