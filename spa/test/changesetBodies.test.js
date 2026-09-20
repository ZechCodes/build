// The hunks behind one changeset's files, fetched a path at a time.
import { describe, expect, it } from "vitest";
import {
  batchPaths,
  createChangesetBodies,
  filePatchesByPath,
  pathOfFilePatch,
  pathsToFetch,
  CHANGESET_DIFF_MAX_PATHS,
} from "../src/core/changesetBodies.js";

const patchFor = (path, line) =>
  [`diff --git a/${path} b/${path}`, "@@ -1 +1 @@", "-old", `+${line}`, ""].join("\n");

describe("reading one answer apart", () => {
  it("names the file a patch is of", () => {
    expect(pathOfFilePatch(patchFor("src/main.rs", "one"))).toBe("src/main.rs");
    expect(pathOfFilePatch("not a patch")).toBeNull();
  });

  it("splits a patch covering several files by path", () => {
    const split = filePatchesByPath(patchFor("a.txt", "one") + patchFor("b.txt", "two"));
    expect([...split.keys()]).toEqual(["a.txt", "b.txt"]);
    expect(split.get("a.txt")).toContain("+one");
    expect(split.get("a.txt")).not.toContain("+two");
  });

  it("answers nothing for a patch that carried nothing", () => {
    expect(filePatchesByPath("").size).toBe(0);
    expect(filePatchesByPath(undefined).size).toBe(0);
  });
});

describe("deciding what to ask for", () => {
  const view = (path, contentKey, rows = null) => ({ path, contentKey, rows });
  const openAll = (views) => new Set(views.map((each) => each.path));

  it("asks for an open file with no body", () => {
    const views = [view("a.txt", "c1")];
    expect(pathsToFetch(views, { openPaths: openAll(views), bodyOf: () => undefined })).toEqual(["a.txt"]);
  });

  it("leaves a file the reader folded shut alone", () => {
    const views = [view("a.txt", "c1"), view("b.txt", "c2")];
    const open = new Set(["a.txt"]);
    expect(pathsToFetch(views, { openPaths: open, bodyOf: () => undefined })).toEqual(["a.txt"]);
  });

  it("asks again for a body that is what the file said before it moved", () => {
    const views = [view("a.txt", "c2")];
    const held = { content_key: "c1", patch: patchFor("a.txt", "old") };
    expect(pathsToFetch(views, { openPaths: openAll(views), bodyOf: () => held })).toEqual(["a.txt"]);
    held.content_key = "c2";
    expect(pathsToFetch(views, { openPaths: openAll(views), bodyOf: () => held })).toEqual([]);
  });

  it("asks nothing of a file that came with its own rows", () => {
    const views = [view("a.txt", "c1", [{ t: "add", text: "one" }])];
    expect(pathsToFetch(views, { openPaths: openAll(views), bodyOf: () => undefined })).toEqual([]);
  });

  it("batches at the cap the verb takes", () => {
    const paths = Array.from({ length: 51 }, (_, index) => `f${index}.txt`);
    const batches = batchPaths(paths);
    expect(batches.map((batch) => batch.length)).toEqual([CHANGESET_DIFF_MAX_PATHS, 1]);
  });
});

describe("createChangesetBodies", () => {
  const layer = (fetchFiles, keyFor = () => "c1") =>
    createChangesetBodies({ addressOf: () => null, fetchFiles, keyFor });

  it("answers each asked path from the one call it makes", async () => {
    const asked = [];
    const bodies = layer(async (paths) => {
      asked.push([...paths]);
      return {
        files: paths.map((path) => ({ path, content_key: `k:${path}` })),
        patch: paths.map((path) => patchFor(path, `${path} body`)).join(""),
      };
    });

    const views = [{ path: "a.txt", contentKey: "k:a.txt" }, { path: "b.txt", contentKey: "k:b.txt" }];
    const filled = await bodies.sync(views, new Set(["a.txt", "b.txt"]));

    expect(asked).toEqual([["a.txt", "b.txt"]]);
    expect(filled).toBe(2);
    expect(bodies.bodyOf("a.txt").patch).toContain("a.txt body");
    expect(bodies.bodyOf("b.txt").content_key).toBe("k:b.txt");
  });

  /** A binary file, or one the changeset stopped touching between the list
   *  and the read: asked for, answered with nothing, and never asked for
   *  again — which is the whole difference between a quiet stack and one that
   *  refetches on every paint. */
  it("files an empty body under the key the list holds", async () => {
    let calls = 0;
    const bodies = layer(async () => {
      calls += 1;
      return { files: [], patch: "" };
    }, () => "c9");

    const views = [{ path: "logo.png", contentKey: "c9" }];
    await bodies.sync(views, new Set(["logo.png"]));
    await bodies.sync(views, new Set(["logo.png"]));

    expect(calls).toBe(1);
    expect(bodies.bodyOf("logo.png")).toEqual({ content_key: "c9", patch: "" });
  });

  it("stops asking once disposed", async () => {
    let calls = 0;
    const bodies = layer(async (paths) => {
      calls += 1;
      return { files: paths.map((path) => ({ path, content_key: "c1" })), patch: patchFor("a.txt", "one") };
    });
    bodies.dispose();

    const filled = await bodies.sync([{ path: "a.txt", contentKey: "c1" }], new Set(["a.txt"]));

    expect(calls).toBe(0);
    expect(filled).toBe(0);
  });
});
