// The device's role-model list, as a panel on its settings page.
//
// A row per model: what it is, which roles it can fill, how much direction it
// needs, and where it sits — because the order is the preference and moving a
// row up is how the user says "prefer this one".
//
// Saves the whole list on every edit. It is small, the ORDER is part of it,
// and a patch protocol for "move this row up" is one nobody could guess from
// the wire.

import { esc } from "./text.js";
import { deviceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";
import {
  AGENT_CAPABILITIES,
  AGENT_ROLES,
  moved,
  rowSummary,
  whyUnsavable,
  withCapability,
  withModel,
  withRole,
  withoutRow,
} from "./agentRoles.js";

/** The panel, empty. The bridge fills it. */
export function agentRolesPanelHtml() {
  return `<div class="panel aroles">
      <h3>🎭 Models and roles</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">Which models fill which roles, and how much direction each needs. An agent making another agent asks for a role and gets your choice — and is told the capability, so it knows whether to write a goal, a scope or a list of steps. Order is preference: when two models can fill a role, the one nearer the top wins.</div>
      <table class="aroles-grid"><thead><tr>
        <th scope="col">Model</th>
        ${AGENT_ROLES.map((role) => `<th scope="col" title="${esc(role.describes)}">${esc(role.label)}</th>`).join("")}
        <th scope="col">Direction needed</th>
        <th scope="col"><span class="visually-hidden">Order</span></th>
      </tr></thead><tbody data-aroles-rows></tbody></table>
      <form class="aroles-add" data-aroles-add>
        <input type="text" name="model" placeholder="claude-opus-5" aria-label="Model id" required>
        <button class="btn mini" type="submit">Add model</button>
      </form>
      <div class="dim" id="arolessaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="aroleserr"></div>
    </div>`;
}

const rowHtml = (row, index, last) => `<tr data-row="${index}" title="${esc(rowSummary(row))}">
    <th scope="row">${esc(row.model)}${row.provider ? ` <span class="dim">${esc(row.provider)}</span>` : ""}</th>
    ${AGENT_ROLES.map(
      (role) => `<td><label class="visually-hidden" for="arole-${index}-${esc(role.id)}">${esc(role.label)}</label>
        <input type="checkbox" id="arole-${index}-${esc(role.id)}" data-role="${esc(role.id)}"${(row.roles || []).includes(role.id) ? " checked" : ""}></td>`,
    ).join("")}
    <td><select data-capability aria-label="Direction ${esc(row.model)} needs">
      ${AGENT_CAPABILITIES.map(
        (entry) =>
          `<option value="${esc(entry.id)}"${entry.id === row.capability ? " selected" : ""}>${esc(entry.label)}</option>`,
      ).join("")}
    </select></td>
    <td class="aroles-order">
      <button class="btn mini" type="button" data-move="-1" aria-label="Prefer ${esc(row.model)} sooner"${index === 0 ? " disabled" : ""}>↑</button>
      <button class="btn mini" type="button" data-move="1" aria-label="Prefer ${esc(row.model)} later"${last ? " disabled" : ""}>↓</button>
      <button class="btn mini danger" type="button" data-remove aria-label="Remove ${esc(row.model)}">×</button>
    </td>
  </tr>`;

/**
 * Mount the panel on one machine's connection.
 *
 * Every edit saves the whole list and repaints from what the bridge answers,
 * so the panel shows what the device actually holds and never what somebody
 * merely tried to put there.
 */
export async function mountAgentRoles(host, { callRpc, deviceId = "", onSaved = async () => {} }) {
  const root = host.querySelector(".aroles");
  if (!root) return;
  const body = root.querySelector("[data-aroles-rows]");
  const saved = root.querySelector("#arolessaved");
  const failed = root.querySelector("#aroleserr");
  let models = [];

  const paint = () => {
    body.innerHTML = models.map((row, index) => rowHtml(row, index, index === models.length - 1)).join("");
  };
  const record = watchSettingsRecord(deviceSettingsAddress(deviceId), (settings) => {
    if (!settings) return;
    models = settings.role_models || [];
    paint();
  }, { owner: root });

  const save = async (next) => {
    const unsavable = whyUnsavable(next);
    if (unsavable) {
      failed.textContent = unsavable;
      return;
    }
    failed.textContent = "";
    try {
      const settings = await callRpc("settings.set", { role_models: next });
      await record.write(settings);
      saved.textContent = "Saved.";
      await onSaved();
    } catch (error) {
      failed.textContent = String(error?.message || error);
      paint();
    }
  };

  body.onchange = (event) => {
    const row = Number(event.target.closest("[data-row]")?.dataset.row);
    if (Number.isNaN(row)) return;
    if (event.target.dataset.role) {
      save(withRole(models, row, event.target.dataset.role, event.target.checked));
    } else if (event.target.matches("[data-capability]")) {
      save(withCapability(models, row, event.target.value));
    }
  };

  body.onclick = (event) => {
    const row = Number(event.target.closest("[data-row]")?.dataset.row);
    if (Number.isNaN(row)) return;
    if (event.target.matches("[data-remove]")) save(withoutRow(models, row));
    else if (event.target.dataset.move) save(moved(models, row, Number(event.target.dataset.move)));
  };

  root.querySelector("[data-aroles-add]").onsubmit = (event) => {
    event.preventDefault();
    const field = event.target.elements.model;
    const model = field.value.trim();
    if (!model) return;
    field.value = "";
    save(withModel(models, { model }));
  };

  try {
    await record.pull(() => callRpc("settings.get"));
  } catch (error) {
    failed.textContent = String(error?.message || error);
  }
  return { dispose: record.dispose };
}
