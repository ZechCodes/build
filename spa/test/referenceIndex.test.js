// #229: one index answers what every written reference names, for the whole
// account, out of the caches the client already holds.
//
// The chat and the task page each built a resolver of their own out of the one
// project they stood in (#63), so a reference to another project's workspace
// stayed prose, and a surface that built none — an activity row, a plan doc, a
// file preview — linked nothing at all. Every surface now asks this one.

import { afterEach, describe, expect, it, vi } from "vitest";

import { referenceTables, resolverFor } from "../src/core/referenceTargets.js";
import {
  holdReferenceSources, referenceIndexVersion, referenceResolver, subscribeReferenceIndex,
} from "../src/core/referenceIndex.js";

const build = { projectKey: "dev-1/proj-1", id: "proj-1", name: "Build" };
const skrift = { projectKey: "dev-1/proj-2", id: "proj-2", name: "Skrift" };
const workspace = (id, name, projectKey, directories = []) => ({
  id, workspace_id: id, name, projectKey, entity_id: `run-${id}`, directories,
});
const feed = {
  projects: [build, skrift],
  workspaces: [
    workspace("ws-1", "tasks-spa", "dev-1/proj-1", [{ source_id: "source-1", name: "Build" }]),
    workspace("ws-2", "skrift-0-2-1-validation", "dev-1/proj-2", [{ source_id: "source-1", name: "smarter-dev" }]),
    workspace("ws-3", "fixes", "dev-1/proj-2", [
      { source_id: "source-1", name: "skrift" }, { source_id: "source-2", name: "skrift-core" },
    ]),
    workspace("ws-4", "shared", "dev-1/proj-1"),
    workspace("ws-5", "shared", "dev-1/proj-2"),
  ],
  items: [{ projectKey: "dev-1/proj-1", entity_id: "run-ws-1", agents: [{ id: "agent-7", name: "Ada" }] }],
};
const tasks = { "dev-1/proj-1": [{ id: "task-42", number: 42, title: "Rebuild the shell" }] };
const here = { deviceId: "dev-1", projectId: "proj-1" };
const at = (place = here, sources = { feed, tasks }) => resolverFor(referenceTables(sources), { place });

describe("what the index answers", () => {
  it("finds a workspace in another project by its name", () => {
    expect(at().workspace("skrift-0-2-1-validation")).toMatchObject({
      deviceId: "dev-1", projectId: "proj-2", workspaceId: "ws-2", name: "skrift-0-2-1-validation",
    });
  });

  it("prefers the reader's own project when two projects share a name", () => {
    expect(at().workspace("shared")?.workspaceId).toBe("ws-4");
    expect(at({ deviceId: "dev-1", projectId: "proj-2" }).workspace("shared")?.workspaceId).toBe("ws-5");
  });

  it("will not guess between two other projects' workspaces", () => {
    expect(at({ deviceId: "dev-1", projectId: "proj-9" }).workspace("shared")).toBeNull();
  });

  it("names a source directory after a slash, by name or source id", () => {
    expect(at().workspace("fixes/skrift-core")).toMatchObject({ workspaceId: "ws-3", sourceId: "source-2" });
    expect(at().workspace("fixes/source-1")).toMatchObject({ workspaceId: "ws-3", sourceId: "source-1" });
    expect(at().workspace("fixes/nowhere")).toBeNull();
  });

  it("reads a whole name before it splits one at a slash", () => {
    const slashed = { ...feed, workspaces: [...feed.workspaces, workspace("ws-6", "fixes/skrift", "dev-1/proj-1")] };
    expect(at(here, { feed: slashed, tasks }).workspace("fixes/skrift")).toMatchObject({ workspaceId: "ws-6" });
    expect(at(here, { feed: slashed, tasks }).workspace("fixes/skrift")?.sourceId).toBeUndefined();
  });

  it("finds a project by name or id, preferring the reader's machine", () => {
    expect(at().project("skrift")).toMatchObject({ deviceId: "dev-1", projectId: "proj-2", name: "Skrift" });
    expect(at().project("proj-1")).toMatchObject({ projectId: "proj-1", name: "Build" });
    expect(at().project("nothing")).toBeNull();
  });

  it("finds an agent by id in whichever workspace it stands", () => {
    expect(at({ deviceId: "dev-1", projectId: "proj-2" }).agent("agent-7")).toMatchObject({
      projectId: "proj-1", workspaceId: "ws-1", agentId: "agent-7",
    });
    expect(at().agent("agent-7").name).toContain("Ada");
    expect(at().agent("agent-gone")).toBeNull();
  });

  it("finds the project's own agent on the reader's project page", () => {
    expect(at().agent("project-01M2")).toMatchObject({ projectId: "proj-1", agentId: "project-01M2", name: "Build agent" });
  });

  it("finds a task in the reader's project", () => {
    expect(at().task(42)).toEqual({ deviceId: "dev-1", projectId: "proj-1", taskId: "task-42", title: "Rebuild the shell" });
    expect(at().task(9999)).toBeNull();
  });
});

// `null` is "held, and not there" — the reader is told. `undefined` is
// "nothing here can say", and the words stay as they were typed.
describe("what the index cannot tell", () => {
  it("cannot place #42 without a project to read it in", () => {
    expect(at(null).task(42)).toBeUndefined();
  });

  it("cannot say a task is missing from a list it never read", () => {
    expect(at({ deviceId: "dev-1", projectId: "proj-2" }).task(1)).toBeUndefined();
  });

  it("cannot say anything before the feed has been read", () => {
    const empty = resolverFor(referenceTables(), { place: here });
    expect(empty.workspace("tasks-spa")).toBeUndefined();
    expect(empty.agent("agent-7")).toBeUndefined();
    expect(empty.project("Build")).toBeUndefined();
  });
});

describe("the index every surface asks", () => {
  afterEach(() => holdReferenceSources({}));

  it("answers from what it was last handed", () => {
    holdReferenceSources({ feed, tasks });
    expect(referenceResolver({ place: here }).task(42)?.taskId).toBe("task-42");
  });

  it("moves its version, and tells its listeners, only when an answer could change", () => {
    holdReferenceSources({ feed, tasks });
    const heard = vi.fn();
    const stop = subscribeReferenceIndex(heard);
    const before = referenceIndexVersion();
    holdReferenceSources({ feed: { ...feed }, tasks: { ...tasks } });
    expect(referenceIndexVersion()).toBe(before);
    expect(heard).not.toHaveBeenCalled();
    holdReferenceSources({ feed, tasks: { ...tasks, "dev-1/proj-2": [] } });
    expect(referenceIndexVersion()).toBe(before + 1);
    expect(heard).toHaveBeenCalledTimes(1);
    stop();
  });
});
