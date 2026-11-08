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

const SHAPES = {
  environment: '<rect x="3" y="3" width="10" height="10" rx="2" fill="none" stroke="currentColor"/>',
  project: '<circle cx="8" cy="8" r="4" fill="currentColor"/>',
  workspace: '<circle cx="8" cy="8" r="4" fill="none" stroke="currentColor"/>',
};

/** One sixteen-pixel slot, with no padding, margin, theme or semantic role. */
export const kindGlyphHtml = (kind) => `<svg class="identity-kind" data-kind="${kind}" width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false">${SHAPES[kind]}</svg>`;

export const projectIdentityHtml = (identity) => `${kindGlyphHtml("project")}${esc(projectIdentityText(identity))}`;
