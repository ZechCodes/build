import { deviceRecord } from "./deviceSync.js";

// Raw device-scope snapshot shapes from config_value -> device_lifecycle.rs
// project_rows, plus the separate project_source/session/directory families.
// No project.list enrichment: the emitted basename is retained, sources are
// separate rows, and diverged/environment_name are absent. Values are synthetic.
export const environmentDeviceSnapshot = {
  status: "ok", scope: { kind: "device" }, incarnation: "environment-device-incarnation",
  bridge_epoch: "environment-device-epoch", watermark: "1", cursor: "environment-device-cursor",
  consistency: "leased", lease: { id: "environment-device-lease", expires_at_ms: 1800000300000 },
  records: [
    ...["proj-1", "proj-2"].flatMap((id, index) => [
      deviceRecord("project", id, { id, project_id: id, name: "build", environment_id: "env-1",
        project_key: index ? "project:second" : "/synthetic/build", home_path: "/synthetic/build", path: "/synthetic/build",
        base_branch: "dev", next_source: 2, added_sources: [], archived_at: null }),
      deviceRecord("project_source", `${id}/source-1`, { project_id: id, id: "source-1", name: "Build",
        path: "/synthetic/build", mount: ".", is_git: true, base_branch: "dev" }),
      deviceRecord("project_session", id, { project_id: id, session_started_ms: 10, last_activity_ms: 1800000000000 + index }),
    ]),
    deviceRecord("workspace", "workspace-1", { id: "workspace-1", project_id: "proj-1", name: "Journal workspace",
      root: "/synthetic/workspace", status: "ready", isolation: "worktree", managed: true, locked: false, created_at: "2026-01-01T00:00:00Z" }),
    deviceRecord("workspace_directory", "workspace-1/dir-1", { id: "dir-1", workspace_id: "workspace-1", project_id: "proj-1",
      source_id: "source-1", name: "Build", path: "/synthetic/workspace/build", is_git: true, status: "ready", branch: "build/journal", source_path: "/synthetic/build", base_branch: "dev" }),
    deviceRecord("badge", "device", { projects: 2, workspaces: 1, locked_workspaces: 0 }),
  ],
};
export const snapshotEnvironments = [{ environment_id: "env-1", name: "Shared environment", project_ids: ["proj-1", "proj-2"],
  sources: [{ id: "source-1", path: "/synthetic/build" }] }];
