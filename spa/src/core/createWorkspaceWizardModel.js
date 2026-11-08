// Wizard targets are a projection of cached rows and local navigation visits.
// Device identity is part of every key, including activity inherited from a
// workspace. Nothing in this model knows a transport or connection state.
import { creationSupportForMethod, environmentWizardSupported } from "./writeOperationProgress.js";
import { deviceKey } from "./deviceKey.js";
import { fuzzyRank } from "./fuzzy.js";
import { RECENT_AFTER_MS } from "./inboxOrdering.js";
import { taskCountText } from "./createWorkspaceTargets.js";

const idOf = (row) => row.id || row.workspace_id || row.project_id || row.environment_id;
const deviceOf = (row) => row.deviceId || row.device_id || "";
const keyOf = (row) => deviceKey(deviceOf(row), idOf(row));
const timeOf = (value) => {
  if (Number.isFinite(value)) return value;
  const parsed = Date.parse(value || "");
  return Number.isFinite(parsed) ? parsed : 0;
};
const rowTime = (row) => Math.max(timeOf(row.created_at), timeOf(row.last_activity_ms));
const latest = (rows, read = rowTime) => Math.max(0, ...rows.map(read));
const byRecency = (left, right) => right.at - left.at;
const sameDevice = (left, right) => deviceOf(left) === deviceOf(right);
const projectWorkspaces = (project, workspaces) => workspaces.filter((row) => sameDevice(row, project) && row.project_id === idOf(project));
const environmentProjects = (environment, projects) => projects.filter((row) => sameDevice(row, environment) && row.environment_id === idOf(environment));
const environmentOf = (project, environments) => environments.find((row) => sameDevice(row, project) && idOf(row) === project.environment_id);
const visitTime = (kind, key, visits) => latest(visits.filter((row) => row.kind === kind && row.key === key), (row) => timeOf(row.at));

function workspaceTime(workspace, visits) {
  return Math.max(rowTime(workspace), visitTime("workspace", keyOf(workspace), visits));
}

function projectTime(project, workspaces, visits) {
  return Math.max(rowTime(project), visitTime("project", keyOf(project), visits), latest(workspaces, (row) => workspaceTime(row, visits)));
}

function deviceFacts(row, devices) {
  const deviceId = deviceOf(row);
  const device = devices.find((item) => item.id === deviceId);
  return {
    deviceId,
    deviceLabel: devices.length > 1 ? device?.name || deviceId : "",
    offline: device?.status === "offline",
  };
}

function targetBase(row, kind, devices) {
  const id = idOf(row);
  const name = row.name || id;
  return { ...row, ...deviceFacts(row, devices), id, kind, key: keyOf(row), name, label: name, summaryLabel: name };
}

function projectLabel(project, environment, siblings) {
  const name = project.name || idOf(project);
  const environmentName = environment?.name || project.environment_name;
  const prefix = environmentName && siblings.length > 1 ? `${environmentName} › ` : "";
  const divergence = project.diverged ? ` +${project.added_sources?.length || 0} folders` : "";
  return `${prefix}${name}${divergence}`;
}

function projectSources(project, environment) {
  if (Array.isArray(project.sources)) return project.sources;
  return [...(environment?.sources || []), ...(project.added_sources || [])];
}

function projectTarget(project, context) {
  const { environments, projects, workspaces, visits, devices } = context;
  const environment = environmentOf(project, environments);
  const siblings = environment ? environmentProjects(environment, projects) : [];
  const children = projectWorkspaces(project, workspaces);
  const base = targetBase(project, "project", devices);
  const taskText = taskCountText(project.open_tasks || 0, project.total_tasks || 0);
  const environmentName = environment?.name || project.environment_name;
  return {
    ...base,
    label: projectLabel(project, environment, siblings),
    summaryLabel: environmentName ? `${environmentName} › ${base.name}` : base.name,
    taskText,
    sources: projectSources(project, environment),
    description: `${project.workspaces ?? children.length} workspaces · ${taskText}`,
    at: projectTime(project, children, visits),
  };
}

function environmentTarget(environment, context, projectTargets) {
  const { projects, workspaces, visits, devices } = context;
  const children = environmentProjects(environment, projects);
  const childKeys = new Set(children.map(keyOf));
  const childTargets = projectTargets.filter((row) => childKeys.has(row.key));
  const childWorkspaces = workspaces.filter((row) => childKeys.has(deviceKey(deviceOf(row), row.project_id)));
  const count = environment.project_ids?.length ?? children.length;
  return {
    ...targetBase(environment, "environment", devices), empty: count === 0, sources: environment.sources || [],
    description: count ? `${count} projects · ${childWorkspaces.length} workspaces` : `no projects yet · ${childWorkspaces.length} workspaces`,
    at: Math.max(rowTime(environment), visitTime("environment", keyOf(environment), visits), latest(childTargets, (row) => row.at)),
  };
}

function presetTarget(options, context, targets) {
  if (options.environmentId) return targets.find(row => row.kind === "environment" && row.id === options.environmentId && row.deviceId === options.deviceId) || null;
  if (!options.projectId || !options.deviceId) return null;
  if (!options.fixedProject && wizardEntrySupported(context,options.deviceId)) return null;
  const key = deviceKey(options.deviceId, options.projectId);
  const held = targets.find((row) => row.key === key);
  return held || projectTarget({ id: options.projectId, deviceId: options.deviceId, name: options.projectName || options.projectId }, context);
}

const section = (kind, title, helper, targets) => ({ kind, title, helper, targets });
const filtered = (targets, query) => fuzzyRank(targets, query, (row) => `${row.name} ${row.deviceLabel}`);

function targetSections(projects, environments, query, now) {
  const recent = [...projects, ...environments].filter((row) => row.at > 0 && now - row.at <= RECENT_AFTER_MS).sort(byRecency).slice(0, 5);
  return [
    section("recent", "Recent", "", filtered(recent, query)),
    section("projects", "Projects", "Uses the existing task board", filtered(projects, query)),
    section("environments", "Environments", "Creates a new task board", filtered(environments, query)),
  ].filter((row) => row.targets.length);
}

function emptyText(sections, query, canCreate) {
  if (sections.length) return "";
  if (query?.trim()) return "No matching projects or environments";
  return canCreate ? "No environments or projects yet" : "Add a project from settings first";
}

function modelContext(options) {
  return {
    projects: (options.projects || []).filter((row) => row.archived_at == null),
    environments: options.environments || [], workspaces: options.workspaces || [],
    devices: options.devices || [], visits: options.visits || [], support: options.support || {}, features:options.features || {},
  };
}

function wizardEntrySupported(context,deviceId) {
  const support=context.features[deviceId];
  return support ? environmentWizardSupported(support):context.support[deviceId] === true;
}
function existingEnvironmentSupported(context, deviceId) {
  const support = context.features[deviceId];
  return support ? creationSupportForMethod(support,"environment.create_project") : context.support[deviceId] === true;
}
export function createWorkspaceWizardModel(options = {}) {
  const context = modelContext(options);
  const projects = context.projects.map((row) => projectTarget(row, context)).sort(byRecency);
  const environments = context.environments.filter((row) => existingEnvironmentSupported(context,deviceOf(row)))
    .map((row) => environmentTarget(row, context, projects)).sort((left, right) => left.name.localeCompare(right.name));
  const sections = targetSections(projects, environments, options.query || "", options.now ?? Date.now());
  const target = presetTarget(options, context, [...projects, ...environments]);
  const legacy = context.devices.some((row) => !wizardEntrySupported(context,row.id));
  return {
    sections, target, page: options.newEnvironment && creationSupportForMethod(options.features?.[options.deviceId],"environment.create") ? 2 : target ? 3 : 1,
    newEnvironmentDevices: context.devices.filter(row => creationSupportForMethod(options.features?.[row.id],"environment.create")),
    emptyText: emptyText(sections, options.query, context.devices.some(row => creationSupportForMethod(options.features?.[row.id],"environment.create"))),
    legacyNote: legacy ? "This device needs an update to create environments here" : "",
  };
}

// Labels describe known references; canonical containment is checked at Create.
export function addedFolderLabel(source, deviceId, snapshot) {
  const location = source.path || source.remote;
  if (!location) return "new folder";
  const references = (rows, sources) => (rows || []).filter(row => deviceOf(row) === deviceId)
    .find(row => sources(row).some(item => (item.path || item.remote) === location));
  const environment = references(snapshot.environments, row => row.sources || []);
  if (environment) return `shared with ${environment.name}`;
  const project = references(snapshot.projects, row => [...(row.sources || []), ...(row.added_sources || [])]);
  return project ? `shared with ${project.name}` : "new folder";
}
