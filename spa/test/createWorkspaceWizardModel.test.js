import { describe, expect, it, vi } from "vitest";
vi.mock("../src/core/createWorkspaceTargets.js", () => ({ taskCountText: (open, total) => open > 0 ? `${open} open tasks` : `${total} tasks` }));
import { createWorkspaceWizardModel, addedFolderLabel } from "../src/core/createWorkspaceWizardModel.js";
const now = Date.parse("2026-11-08T01:00:00Z");
const device = { id: "d1", name: "Laptop", status: "online" };
const env = { environment_id: "e1", deviceId: "d1", name: "Build", sources: [{ path: "/code/build" }] };
const project = { id: "p1", deviceId: "d1", name: "Ship wizard", environment_id: "e1", open_tasks: 32, total_tasks: 102 };
const base = { now, devices: [device], environments: [env], projects: [project], support: { d1: true } };
const section = (model, kind) => model.sections.find((row) => row.kind === kind);

describe("create workspace target model", () => {
  it("lists existing boards before environments with persistent helpers", () => {
    const model = createWorkspaceWizardModel(base);
    expect(model.page).toBe(1);
    expect(model.sections.map((row) => row.kind)).toEqual(["projects", "environments"]);
    expect(section(model, "projects").helper).toBe("Uses the existing task board");
    expect(section(model, "environments").helper).toBe("Creates a new task board");
    expect(section(model, "projects").targets[0]).toMatchObject({ kind: "project", key: "d1/p1", label: "Ship wizard", taskText: "32 open tasks", sources: env.sources });
    expect(section(model, "environments").targets[0]).toMatchObject({ kind: "environment", id: "e1", description: "1 projects · 0 workspaces" });
  });
  it("propagates workspace activity and visits to recent projects and environments", () => {
    const model = createWorkspaceWizardModel({ ...base, workspaces: [{ workspace_id: "w1", deviceId: "d1", project_id: "p1", last_activity_ms: now - 3000 }], visits: [{ kind: "workspace", key: "d1/w1", at: now - 1000 }] });
    expect(model.sections[0].kind).toBe("recent");
    expect(section(model, "recent").targets).toEqual(expect.arrayContaining([expect.objectContaining({ kind: "project", at: now - 1000 }), expect.objectContaining({ kind: "environment", at: now - 1000 })]));
  });
  it("caps recents at five newest targets and keeps projects sorted by recency", () => {
    const projects = Array.from({ length: 8 }, (_, index) => ({ ...project, id: `p${index}`, created_at: new Date(now - index * 1000).toISOString() }));
    projects.push({ ...project, id: "old", created_at: new Date(now - 86400001).toISOString() });
    const model = createWorkspaceWizardModel({ ...base, projects });
    const recent = section(model, "recent").targets;
    expect(recent).toHaveLength(5);
    expect(recent.map((row) => row.at)).toEqual(recent.map((row) => row.at).sort((a, b) => b - a));
    expect(recent.some((row) => row.id === "old")).toBe(false);
    expect(section(model, "projects").targets.map((row) => row.id)).toEqual(projects.map((row) => row.id));
  });
  it("keeps device-qualified activity and labels every target on a multiple-device account", () => {
    const model = createWorkspaceWizardModel({ ...base, devices: [device, { id: "d2", name: "Desktop", status: "offline" }], projects: [project, { ...project, deviceId: "d2" }], environments: [env, { ...env, deviceId: "d2" }], support: { d1: true, d2: true }, visits: [{ kind: "project", key: "d2/p1", at: now - 50 }] });
    expect(section(model, "projects").targets[0]).toMatchObject({ deviceId: "d2", deviceLabel: "Desktop", offline: true, at: now - 50 });
    expect(section(model, "projects").targets[1]).toMatchObject({ deviceId: "d1", deviceLabel: "Laptop", offline: false, at: 0 });
    expect(section(model, "recent").targets.every((row) => row.deviceId === "d2")).toBe(true);
    expect(section(createWorkspaceWizardModel(base), "projects").targets[0].deviceLabel).toBe("");
  });
  it("shows the M1 empty state and environments with no projects", () => {
    const empty = createWorkspaceWizardModel({ now });
    expect(empty.sections).toEqual([]);
    expect(empty.emptyText).toBe("Add a project from settings first");
    const model = createWorkspaceWizardModel({ ...base, projects: [] });
    expect(model.sections.map((row) => row.kind)).toEqual(["environments"]);
    expect(section(model, "environments").targets[0].description).toContain("no projects yet");
    expect(JSON.stringify(empty)).not.toContain("New environment…");
  });
  it("falls back to current-project creation for an old bridge", () => {
    const model = createWorkspaceWizardModel({ ...base, support: {}, projectId: "p1", deviceId: "d1" });
    expect(model.page).toBe(3);
    expect(model.target).toMatchObject({ kind: "project", id: "p1" });
    expect(model.sections.some((row) => row.kind === "environments")).toBe(false);
    expect(model.legacyNote).toBe("This device needs an update to create environments here");
    expect(createWorkspaceWizardModel({ ...base, support: {} }).sections.map((row) => row.kind)).toEqual(["projects"]);
  });
  it("presets fixed-project mode even before cache has listed the project", () => {
    const fixed = createWorkspaceWizardModel({ ...base, projectId: "p1", deviceId: "d1", fixedProject: true });
    expect(fixed.page).toBe(3);
    expect(fixed.target).toMatchObject({ id: "p1", summaryLabel: "Build › Ship wizard" });
    const cold = createWorkspaceWizardModel({ projectId: "p9", deviceId: "d1", projectName: "Payments", fixedProject: true });
    expect(cold.target).toMatchObject({ id: "p9", deviceId: "d1", name: "Payments", kind: "project" });
  });
  it("uses count text and sibling prefixes, workspace counts and divergence", () => {
    const model = createWorkspaceWizardModel({ ...base, projects: [{ ...project, open_tasks: 0 }, { ...project, id: "p2", name: "Second", open_tasks: 0, total_tasks: 0, diverged: true, added_sources: [{ path: "/extra" }] }], workspaces: [{ id: "w", deviceId: "d1", project_id: "p1" }] });
    const targets = section(model, "projects").targets;
    expect(targets[0]).toMatchObject({ label: "Build › Ship wizard", description: "1 workspaces · 102 tasks", taskText: "102 tasks" });
    expect(targets[1]).toMatchObject({ label: "Build › Second +1 folders", taskText: "0 tasks" });
    expect(section(model, "environments").targets[0].description).toBe("2 projects · 1 workspaces");
  });
  it("omits archived projects and ranks fuzzy results inside their sections", () => {
    const model = createWorkspaceWizardModel({ ...base, query: "bd", projects: [project, { ...project, id: "p2", name: "Build task" }, { ...project, id: "p3", name: "Build archived", archived_at: now }], environments: [{ ...env, environment_id: "e2", name: "Zoo" }, env] });
    expect(model.sections.map((row) => row.kind)).toEqual(["projects", "environments"]);
    expect(section(model, "projects").targets.map((row) => row.id)).toEqual(["p2"]);
    expect(section(model, "environments").targets.map((row) => row.id)).toEqual(["e1"]);
  });
  it("distinguishes a search miss from having no targets", () => {
    const model = createWorkspaceWizardModel({ ...base, query: "zzzzzzzz" });
    expect(model.sections).toEqual([]);
    expect(model.emptyText).toBe("No matching projects or environments");
  });
  it("sorts environments alphabetically and accepts wire ids and effective sources", () => {
    const sources = [{ path: "/effective" }];
    const model = createWorkspaceWizardModel({ ...base, environments: [{ ...env, environment_id: "e2", name: "Zebra" }, { ...env, name: "Alpha" }], projects: [{ ...project, id: undefined, project_id: "p1", sources }] });
    expect(section(model, "environments").targets.map((row) => row.name)).toEqual(["Alpha", "Zebra"]);
    expect(section(model, "projects").targets[0]).toMatchObject({ id: "p1", sources });
  });
});

it("offers new environments only from cached create support and presets environment shortcuts", () => {
  const options = { ...base, features: { d1: { create: true, createProject: true, addedSources: true } } };
  expect(createWorkspaceWizardModel(options).newEnvironmentDevices).toEqual([device]);
  expect(createWorkspaceWizardModel({ ...options, environmentId: "e1", deviceId: "d1" })).toMatchObject({ page: 3, target: { id: "e1", kind: "environment" } });
  expect(createWorkspaceWizardModel({ ...options, newEnvironment: true, deviceId: "d1" }).page).toBe(2);
  expect(createWorkspaceWizardModel(base).newEnvironmentDevices).toEqual([]);
});
it("labels advanced folders by same-device shared references or as a new folder", () => {
  expect(addedFolderLabel({path:"/code/build"}, "d1", base)).toBe("shared with Build");
  expect(addedFolderLabel({path:"/extra"}, "d1", {...base, projects:[{...project, added_sources:[{path:"/extra"}]}]})).toBe("shared with Ship wizard");
  expect(addedFolderLabel({path:"/extra"}, "d2", base)).toBe("new folder");
});
it("marks empty environments resumable while keeping their no-project description", () => {
  const row = section(createWorkspaceWizardModel({...base,projects:[]}),"environments").targets[0];
  expect(row).toMatchObject({empty:true}); expect(row.description).toContain("no projects yet");
});
it("replaces the M1 add-project empty state when a cached device can create environments",()=>{
 const model=createWorkspaceWizardModel({devices:[device],support:{d1:true},features:{d1:{create:true}}});
 expect(model.emptyText).not.toContain("Add a project");expect(model.newEnvironmentDevices).toEqual([device]);
});

it("archived projects are hidden from the wizard", () => {
  const archived = { ...project, archived_at: 1 };
  const model = createWorkspaceWizardModel({ ...base, projects: [archived], fixedProject: true, deviceId: "d1", projectId: "p1" });
  expect(model.sections.flatMap(section => section.targets).some(row => row.kind === "project")).toBe(false);
  expect(model.target).toBeNull();
});
