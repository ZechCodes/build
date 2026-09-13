// The two version gates: what the app shows when it and the bridge on the
// other end no longer speak a common API major.
//
// `selectAdapter` (core/bridgeApi/index.js) answers `{unsupported}` when no
// adapter claims the bridge's `api_version`, naming the side that needs
// updating. Each side gets a screen with one instruction and nothing else —
// no surface behind it, because every surface would be guessing at a shape.
//
// These render into an element and return nothing; wiring them to the
// connection gate (views/gate.js) is the caller's job, as is deciding when the
// served-version watcher has something newer to reload onto.

import { esc } from "../core/text.js";

const PANEL_STYLE = "max-width:680px;margin:44px auto 0;padding:0 16px";

function gateHtml({ heading, blurb, body }) {
  return `
    <div style="${PANEL_STYLE}">
      <h1 style="margin:0 0 6px">${heading}</h1>
      <p class="settings-intro" style="margin:0 0 18px">${blurb}</p>
      <div class="panel">${body}</div>
    </div>`;
}

const versionLine = (bridgeVersion) =>
  `<div class="row"><span class="k">Bridge API</span><span class="v mono">${esc(String(bridgeVersion || "unknown"))}</span></div>`;

/**
 * The bridge speaks an API major above every adapter this build carries: the
 * tab is old. The reload is the served-version watcher's — pass `onReload`
 * only when there is something newer to reload onto, so the button never
 * promises a refresh that lands on the same bundle.
 */
export function renderAppBehindBridgeGate(root, { deviceName, bridgeVersion, onReload } = {}) {
  const device = esc(String(deviceName || "this device"));
  const reload = onReload
    ? '<div class="row" style="margin-top:12px"><button class="btn primary" id="gate-reload">Reload the app</button></div>'
    : '<div class="dim" style="font-size:12.5px;margin-top:12px">The updated app is not being served yet — this page will offer a reload as soon as it is.</div>';
  root.innerHTML = gateHtml({
    heading: `This app is behind the bridge on ${device}`,
    blurb: `The bridge on ${device} speaks a newer version of the Build API than this tab knows. Reload to pick up the current app.`,
    body: `${versionLine(bridgeVersion)}${reload}`,
  });
  const button = root.querySelector("#gate-reload");
  if (button) button.onclick = () => onReload();
}

/**
 * The bridge speaks an API major below every adapter this build carries: the
 * machine's bridge is old. `installCommand` is the one from the downloads
 * route — the same line the onboarding screen hands out.
 */
export function renderBridgeBehindAppGate(
  root,
  { deviceName, bridgeVersion, installCommand, clipboard = globalThis.navigator?.clipboard } = {},
) {
  const device = esc(String(deviceName || "this device"));
  const command = String(installCommand || "");
  const commandBlock = command
    ? `<div class="row" style="margin-top:12px"><span class="v mono" style="font-size:11.5px;word-break:break-all">${esc(command)}</span>
       <button class="btn" id="gate-install-copy" style="margin-left:auto">Copy</button></div>`
    : '<div class="dim" style="font-size:12.5px;margin-top:12px">Run the Build installer on that machine again to update its bridge.</div>';
  root.innerHTML = gateHtml({
    heading: `The bridge on ${device} needs updating`,
    blurb: `This app speaks a newer version of the Build API than the bridge on ${device}. Update the bridge and it will reconnect on its own.`,
    body: `${versionLine(bridgeVersion)}${commandBlock}`,
  });
  const button = root.querySelector("#gate-install-copy");
  if (button) button.onclick = () => clipboard?.writeText(command);
}
