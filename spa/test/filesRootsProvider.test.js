import { describe, expect, it } from "vitest";
import { projectFilesRoots, workspaceFilesRoots } from "../src/core/filesRoots.js";

describe("Files roots", () => {
  it("keeps bodies and listings from a moved source folder in separate caches", () => {
    const roots = (path) => projectFilesRoots({ sources: [{ id: "docs", path }] }, "p");
    expect(roots("/old")[0].cacheEntityId).toBe('project:["p","docs","/old"]');
    expect(roots("/new")[0].cacheEntityId).not.toBe(roots("/old")[0].cacheEntityId);
    expect(roots("/new")[0].id).toBe("docs");
  });
  it("takes project source identities and labels in their order", () => {
    expect(projectFilesRoots({ sources: [{ id: "code", name: "Code" }, { id: "docs", mount: "docs" }] }, "p")).toEqual([
      { id: "code", label: "Code", scope: { project_id: "p", source_id: "code" } },
      { id: "docs", label: "docs", scope: { project_id: "p", source_id: "docs" } },
    ]);
    expect(projectFilesRoots({ sources: [] }, "p")).toEqual([]);
  });
  it("shares the workspace directory provider without changing owned git scopes", () => {
    expect(workspaceFilesRoots({ entity_id: "run", directories: [{ source_id: "code", name: "Code", is_git: true }, { source_id: "docs", mount: "docs" }] }, "w")).toEqual([
      { id: "code", label: "Code", scope: { workspace_id: "w", source_id: "code", entity_id: "run" } },
      { id: "docs", label: "docs", scope: { workspace_id: "w", source_id: "docs" } },
    ]);
  });
  it("gives legacy projects one primary root", () => {
    expect(projectFilesRoots({ name: "Build", path: "/code/build" }, "p")).toEqual([{ id: "primary", label: "Build", scope: { project_id: "p" } }]);
    expect(projectFilesRoots(null, "p")).toEqual([]);
  });
});
