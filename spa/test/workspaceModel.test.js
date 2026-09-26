import { describe, expect, it } from "vitest";
import { workspaceRoute } from "../src/core/projectModel.js";
import { directoryId, selectedDirectory, workspaceDirectoryModel, workspaceScope } from "../src/core/workspaceModel.js";

const workspace = {
  id: "ws-1",
  project_id: "p-1",
  directories: [
    { id: "docs", name: "Docs", is_git: false },
    { source_id: "app", name: "App", is_git: true },
  ],
};

describe("workspace navigation model", () => {
  it("lands on the first repository when the workspace contains one", () => {
    expect(workspaceRoute(workspace)).toEqual({
      name: "workspace", projectId: "p-1", workspaceId: "ws-1", sourceId: "app", tab: "changes",
    });
  });

  it("lands ordinary directories in Files", () => {
    expect(workspaceRoute({ ...workspace, directories: [workspace.directories[0]] })).toMatchObject({ sourceId: "docs", tab: "files" });
  });

  it("supports an empty workspace and rejects malformed identities", () => {
    expect(workspaceRoute({ ...workspace, directories: [] })).toEqual({
      name: "workspace", projectId: "p-1", workspaceId: "ws-1", tab: "changes",
    });
    expect(workspaceRoute({ id: "ws-1" })).toBeNull();
    expect(workspaceRoute(null)).toBeNull();
  });

  it("normalizes source ids and selects a requested directory", () => {
    expect(directoryId(workspace.directories[1])).toBe("app");
    expect(selectedDirectory(workspace, "docs")).toBe(workspace.directories[0]);
    expect(selectedDirectory(workspace, "missing")).toBe(workspace.directories[0]);
    expect(workspaceScope("ws-1", "app")).toEqual({ workspace_id: "ws-1", source_id: "app" });
  });

  it("turns workspace directories into persistent tab identities, in the workspace's order", () => {
    expect(workspaceDirectoryModel(workspace, "app").map((row) => [row.sourceId, row.label, row.current])).toEqual([
      ["docs", "Docs", false],
      ["app", "App", true],
    ]);
  });
});
