// Where a file reference in a conversation points.
//
// A reference is written by an agent working in a checkout, so the path is
// relative to that checkout and nothing on the wire says which surface should
// open it: the conversation's own context does. One helper answers, and both
// the chip's href and the click that follows it read the same answer.

import { describe, it, expect } from "vitest";

import { fileLinkRoute } from "../src/core/threadLinks.js";
import { hashFromRoute } from "../src/core/router.js";

const workspaceContext = (over = {}) => ({
  kind: "workspace",
  deviceId: "dev-1",
  projectId: "p1",
  workspaceId: "ws-1",
  sourceId: "repo",
  directories: [{ source_id: "repo", name: "Repository" }],
  branch: "build/login",
  ...over,
});

const branchContext = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  projectId: "p1",
  branch: "build/login",
  ...over,
});

const TWO_DIRECTORIES = [
  { source_id: "repo", name: "Repository" },
  { source_id: "assets", name: "Assets" },
];

describe("the route a file reference names", () => {
  it("opens the workspace's Files tab on the line the reference starts at", () => {
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js", line_start: 8, line_end: 12 }, workspaceContext())).toEqual({
      name: "workspace",
      deviceId: "dev-1",
      projectId: "p1",
      workspaceId: "ws-1",
      sourceId: "repo",
      tab: "files",
      file: "src/parser.js",
      line: 8,
    });
  });

  it("carries no line for a reference that names none", () => {
    const route = fileLinkRoute({ kind: "file", path: "src/parser.js" }, workspaceContext());
    expect(route).toMatchObject({ file: "src/parser.js", tab: "files" });
    expect("line" in route).toBe(false);
  });

  it("opens the branch's Files tab at the file, not at the tab's root", () => {
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js", line_start: 3 }, branchContext())).toEqual({
      name: "branch",
      deviceId: "dev-1",
      projectId: "p1",
      branch: "build/login",
      tab: "files",
      file: "src/parser.js",
      line: 3,
    });
  });

  it("follows a multi-directory path into the directory it is mounted under", () => {
    const context = workspaceContext({ directories: TWO_DIRECTORIES });
    expect(fileLinkRoute({ kind: "file", path: "Assets/logo.svg", line_start: 2 }, context)).toMatchObject({
      sourceId: "assets",
      file: "logo.svg",
      line: 2,
    });
  });

  it("keeps a multi-directory path that names no directory in the open one", () => {
    const context = workspaceContext({ directories: TWO_DIRECTORIES });
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js" }, context)).toMatchObject({
      sourceId: "repo",
      file: "src/parser.js",
    });
  });

  // With one directory the agent's paths are written from its root, so a first
  // segment that happens to be named like the directory is a real folder in it.
  it("strips nothing from a path in a workspace of one directory", () => {
    const context = workspaceContext({ directories: [{ source_id: "repo", name: "Repository" }] });
    expect(fileLinkRoute({ kind: "file", path: "Repository/readme.md" }, context)).toMatchObject({
      sourceId: "repo",
      file: "Repository/readme.md",
    });
  });

  it("leaves a bare directory name as the path it is, having no file under it", () => {
    const context = workspaceContext({ directories: TWO_DIRECTORIES });
    expect(fileLinkRoute({ kind: "file", path: "Assets" }, context)).toMatchObject({
      sourceId: "repo",
      file: "Assets",
    });
  });

  it("matches a directory by its source id when the record carries no name", () => {
    const context = workspaceContext({
      directories: [{ source_id: "repo" }, { source_id: "assets" }],
    });
    expect(fileLinkRoute({ kind: "file", path: "assets/logo.svg" }, context)).toMatchObject({
      sourceId: "assets",
      file: "logo.svg",
    });
  });

  it("names no route for a reference that is not a file", () => {
    expect(fileLinkRoute({ kind: "plan_stage", path: ".build/plan/01-parser.md" }, workspaceContext())).toBe(null);
  });

  it("names no route where the conversation is about neither a workspace nor a branch", () => {
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js" }, { kind: "issue", projectId: "p1" })).toBe(null);
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js" }, branchContext({ branch: null }))).toBe(null);
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js" }, workspaceContext({ workspaceId: null }))).toBe(null);
    expect(fileLinkRoute({ kind: "file", path: "src/parser.js" }, null)).toBe(null);
    expect(fileLinkRoute(null, workspaceContext())).toBe(null);
  });

  it("writes a URL the router reads back as the same place", () => {
    const route = fileLinkRoute({ kind: "file", path: "src/parser.js", line_start: 8 }, workspaceContext());
    expect(hashFromRoute(route)).toBe(
      "#/device/dev-1/project/p1/workspace/ws-1/directory/repo/files?path=src%2Fparser.js&line=8",
    );
  });
});
