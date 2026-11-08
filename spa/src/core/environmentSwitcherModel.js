import { navigationPickerModel } from "./navigationModel.js";
import { environmentKeyOf, environmentNavigationSupported, projectEnvironmentIdentity } from "./environmentIdentity.js";
import { deviceKey } from "./deviceKey.js";
import { lastVisitedDestinations } from "./navigationVisitModel.js";

export const navigationTime = (value) => {
  if (Number.isFinite(value)) return value;
  return Date.parse(value || "") || 0;
};
const latest = (times) => Math.max(0, ...times);
const rowTime = (row) => Math.max(navigationTime(row.last_activity_ms), navigationTime(row.created_at));
const visitTime = (row, visits) => latest(visits.filter((visit) => visit.kind === row.kind && visit.key === row.key).map((visit) => navigationTime(visit.at)));
const byRecency = (left, right) => right.at - left.at || left.name.localeCompare(right.name) || left.key.localeCompare(right.key);
const archived = (row) => !!(row.archived || row.archived_at);

function addedFolderCount(project, environments = []) {
  // M1 assumes the cache is current: an undetectably older environment can count a new inherited source until the next refresh.
  if (Array.isArray(project.added_sources)) return project.added_sources.length;
  if (typeof project.diverged !== "boolean" || !Array.isArray(project.sources)) return null;
  const key = deviceKey(project.deviceId, project.environment_id);
  const environment = environments.find(row => environmentKeyOf(row) === key);
  if (!Array.isArray(environment?.sources)) return null;
  const projectIds = new Set(project.sources.map(source => source.id));
  if (!environment.sources.every(source => projectIds.has(source.id))) return null;
  const inherited = new Set(environment.sources.map(source => source.id));
  return project.diverged ? project.sources.filter(source => !inherited.has(source.id)).length : 0;
}

function projectDestination(group, visits, environments) {
  const at = Math.max(rowTime(group.destination.project), visitTime(group.destination, visits), latest(group.workspaces.map((row) =>
    Math.max(rowTime(row.workspace), visitTime(row, visits)))));
  return { ...group.destination, unreadCount: group.unreadCount, at,
    addedFolders: addedFolderCount(group.destination.project, environments), archived: archived(group.destination.project) };
}

function environmentGroups(picker, catalog, options) {
  const projects = catalog.projects.map((group) => group.destination.project);
  const groups = new Map();
  for (const group of picker.groups) {
    const identity = projectEnvironmentIdentity(group.destination.project, projects, options);
    if (!identity) { groups.set(group.key, group); continue; }
    const destinations = environmentDestinations(group, options);
    if (!destinations.length) continue;
    if (!groups.has(identity.key)) groups.set(identity.key, {
      kind: "environment", key: identity.key, name: identity.environmentName,
      deviceId: group.deviceId, deviceLabel: identity.deviceLabel, destinations: [],
    });
    groups.get(identity.key).destinations.push(...destinations);
  }
  return [...groups.values()].map((group) => group.kind === "environment"
    ? { ...group, destinations: group.destinations.sort(byRecency) } : group)
    .sort((left, right) => left.name.localeCompare(right.name) || left.key.localeCompare(right.key));
}

function environmentDestinations(group, options) {
  const project = projectDestination(group, options.visits || [], options.environments);
  if (!String(options.query || "").trim()) return project.archived ? [] : [project];
  return group.destinations.map((row) => row.kind === "project" ? project : {
    ...row, at: rowTime(row.workspace), projectName: group.name, deviceLabel: group.deviceName,
  });
}

/** Legacy groups pass through intact on old devices, including in mixed accounts. */
export function switcherModel(catalog, options = {}) {
  const picker = navigationPickerModel(catalog, options);
  const visits = options.visits || [];
  const projects = catalog.projects.filter((group) => environmentNavigationSupported(options, group.deviceId))
    .map((group) => projectDestination(group, visits, options.environments)).filter((row) => !row.archived);
  const now = options.nowMs ?? Date.now();
  const recent = projects.filter((row) => row.at > 0 && now - row.at <= 24 * 3600_000).sort(byRecency).slice(0, 5);
  return { ...picker, groups: environmentGroups(picker, catalog, options),
    lastVisited: String(options.query || "").trim() ? [] : recent,
    legacyLastVisited: lastVisitedDestinations(catalog, options, row => !environmentNavigationSupported(options, row.deviceId)),
    environmentGrouped: true };
}
