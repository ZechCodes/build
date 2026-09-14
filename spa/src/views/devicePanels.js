// The panels a machine's own settings page stands up: the projects it holds
// and the settings its bridge owns.
//
// Every one of them is an answer from that one machine, so they are all mounted
// on the page's own connection and none of them exists until the machine is
// answering. Which panels there are is a list, so adding another is a line
// here and no page learns the order by hand.

import { contextFor } from "../core/deviceContexts.js";
import { refreshFeed } from "../core/taskFeed.js";
import { deviceProjectsPanelHtml, mountDeviceProjects } from "./deviceProjects.js";
import { agentModesPanelHtml, mountAgentModes } from "../core/agentModes.js";
import { defaultHarnessPanelHtml, mountDefaultHarness } from "../core/defaultHarness.js";
import { DEVICE_ISOLATION, isolationPanelHtml, mountIsolation } from "../core/isolation.js";
import { mountTriageSetting, triageSettingPanelHtml } from "../core/triageSetting.js";

/** The panels this machine's bridge owns: each states its own markup and mounts
 *  itself on the page's connection, so they are stood up in one pass. */
const BRIDGE_PANELS = [
  { html: agentModesPanelHtml, mount: mountAgentModes },
  { html: defaultHarnessPanelHtml, mount: mountDefaultHarness },
  { html: isolationPanelHtml, mount: (host, options) => mountIsolation(host, { ...options, target: DEVICE_ISOLATION }) },
  { html: triageSettingPanelHtml, mount: mountTriageSetting },
];

/** The account's copy of what this machine offers is what these panels have
 *  just changed, so the registry is told to ask again. Guarded twice over: the
 *  device page opens its own connection and may be looking at a machine the app
 *  holds no context for, and a refused re-read is the catalog's business, not
 *  this save's — the panel has its confirmation either way. */
async function refreshAccountCatalog(deviceId) {
  try {
    await contextFor(deviceId)?.refreshModelCatalog();
  } catch {
    // The next surface that asks this machine for its harnesses reads it again.
  }
}

/**
 * Paint and wire every panel about `device`, on `callRpc`.
 *
 * The markup is rebuilt on each call, so a reconnect starts from what the
 * machine says now and not from the last connection's refusal.
 */
export async function standUpDevicePanels({ projectsHost, bridgeHost, callRpc, device }) {
  projectsHost.innerHTML = deviceProjectsPanelHtml();
  bridgeHost.innerHTML = BRIDGE_PANELS.map((panel) => panel.html()).join("");
  await mountDeviceProjects(projectsHost, {
    callRpc,
    deviceName: device.name,
    // The rail is where the new project is looked for next: the app's own
    // context for this machine reads it again. It has none while the machine
    // has never answered the app itself, and then there is nothing to re-read.
    onProjectCreated: () => refreshFeed(device.id),
  });
  const options = { callRpc, onSaved: () => refreshAccountCatalog(device.id) };
  for (const panel of BRIDGE_PANELS) await panel.mount(bridgeHost, options);
}
