// Cached, device-qualified identity shared by navigation and inbox renderers.
import { deviceKey } from "./deviceKey.js";
import { esc } from "./text.js";

export const environmentIdOf = (row) => row.environment_id || row.id;
export const environmentKeyOf = (row) => deviceKey(row.deviceId, environmentIdOf(row));
export const environmentNavigationSupported = (context, deviceId) => context?.support?.[deviceId] === true;

export function identityDeviceLabel(row, devices = []) {
  const device = devices.find((device) => device.id === row.deviceId);
  if (device?.status === "offline") return "Offline";
  return devices.length > 1 ? device?.name || row.device || row.deviceId : "";
}

export function projectEnvironmentIdentity(project, projects = [], context = {}) {
  if (!project?.environment_id || !environmentNavigationSupported(context, project.deviceId)) return null;
  const key = deviceKey(project.deviceId, project.environment_id);
  const environment = (context.environments || []).find((row) => environmentKeyOf(row) === key);
  const siblings = projects.filter((row) => row.deviceId === project.deviceId && row.environment_id === project.environment_id);
  return environmentProjectIdentity(project, environment, siblings, context);
}

function environmentProjectIdentity(project, environment, siblings, context) {
  const count = Math.max(environment?.project_ids?.length || 0, siblings.length);
  return {
    key: deviceKey(project.deviceId, project.environment_id), environmentName: environment?.name || project.environment_name || project.environment_id,
    projectName: project.name || project.id, showEnvironment: count > 1,
    deviceLabel: identityDeviceLabel(project, context.devices),
  };
}

export const projectIdentityText = (identity) => identity.showEnvironment
  ? `${identity.environmentName} › ${identity.projectName}` : identity.projectName;

/** FNV-1a over the device-qualified environment key: stable across tabs/reloads. */
function badgeHue(key) {
  let hash = 2166136261;
  for (const character of key) hash = Math.imul(hash ^ character.codePointAt(0), 16777619);
  return (hash >>> 0) % 12 + 1;
}

function badgeLetters(name) {
  const letters = String(name || "").match(/\p{L}/gu) || [];
  return letters.slice(0, 2).map(letter => {
    const uppercase = letter.toUpperCase();
    return Array.from(uppercase).length === 1 ? uppercase : letter;
  }).join("") || "?";
}

/** One existing sixteen-pixel SVG slot; cached identity alone determines paint.
 * Generated content paints the letters without adding DOM text to labels.
 * key is deviceKey(deviceId, environmentId), never a project/workspace key. */
export function identityBadgeHtml({ key, name }, kind = "project") {
  const hue = badgeHue(key);
  return `<svg class="identity-kind identity-badge" data-kind="${esc(kind)}" data-hue="${hue}" style="--identity-hue:var(--badge-${hue})" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false"><rect width="16" height="16" rx="3"></rect><foreignObject width="16" height="16"><span xmlns="http://www.w3.org/1999/xhtml" class="identity-letters" data-glyph="${esc(badgeLetters(name))}"></span></foreignObject></svg>`;
}

/** Rows without an environment (legacy projects or unsubmitted drafts) use
 * their device-qualified row id until a persisted environment exists. */
export const rowIdentityBadgeHtml = (row, kind = row.kind) => identityBadgeHtml({
  key: deviceKey(row.deviceId, row.environment_id || row.id || row.project_id), name: row.name,
}, kind);

export const projectIdentityHtml = (identity) => `${identityBadgeHtml({ key: identity.key, name: identity.projectName })}${esc(projectIdentityText(identity))}`;
