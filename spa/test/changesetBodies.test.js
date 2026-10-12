// The hunks behind one changeset's files, fetched a path at a time.
import { describe, expect, it } from "vitest";
import {
  batchPaths,
  createChangesetBodies,
  filePatchesByPath,
  pathOfFilePatch,
  pathsToFetch,
  BODIES_PER_TURN,
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

  it("fetches paths a truncated aggregate never reached instead of keeping empty bodies", async () => {
    const asked = [];
    const paths = ["a.txt", "b.txt", "c.txt", "d.txt"];
    const files = paths.map((path) => ({ path, content_key: "c1" }));
    const bodies = layer(async (wanted) => {
      asked.push([...wanted]);
      return { files, patch: wanted.map((path) => patchFor(path, "from wire")).join("") };
    });
    await bodies.seed({ files, patch: patchFor("a.txt", "whole") + patchFor("b.txt", "cut"), truncated: true }, paths);
    const views = files.map((file) => ({ path: file.path, contentKey: file.content_key }));
    await bodies.sync(views, new Set(paths));
    await bodies.sync(views, new Set(paths));

    expect(asked).toEqual([["c.txt", "d.txt"]]);
    for (const path of ["c.txt", "d.txt"]) expect(bodies.bodyOf(path).patch).toContain("+from wire");
    bodies.dispose();
  });

  it("retries paths omitted by a truncated wire answer on the next paint", async () => {
    const asked = [];
    const bodies = layer(async (paths) => {
      asked.push([...paths]);
      if (asked.length === 1) return { files: [], patch: patchFor("a.txt", "cut"), truncated: true };
      return { files: [], patch: patchFor("b.txt", "retried") };
    });
    const views = ["a.txt", "b.txt"].map((path) => ({ path, contentKey: "c1" }));
    const open = new Set(views.map((view) => view.path));
    await bodies.sync(views, open);
    await bodies.sync(views, open);

    expect(asked).toEqual([["a.txt", "b.txt"], ["b.txt"]]);
    expect(bodies.bodyOf("b.txt").patch).toContain("+retried");
    bodies.dispose();
  });

  it("retries the same paths after a rejected fill", async () => {
    const asked = [];
    const bodies = layer(async (paths) => {
      asked.push([...paths]);
      if (asked.length === 1) throw new Error("transport hiccup");
      return { files: [], patch: patchFor("a.txt", "recovered") };
    });
    const views = [{ path: "a.txt", contentKey: "c1" }];
    const open = new Set(["a.txt"]);
    await expect(bodies.sync(views, open)).rejects.toThrow("transport hiccup");
    await bodies.sync(views, open);

    expect(asked).toEqual([["a.txt"], ["a.txt"]]);
    expect(bodies.bodyOf("a.txt").patch).toContain("+recovered");
    bodies.dispose();
  });

  it("fetches an unanswered path even when its list has no content key", async () => {
    const bodies = layer(async () => ({ files: [], patch: patchFor("a.txt", "unkeyed") }), () => undefined);
    await bodies.sync([{ path: "a.txt" }], new Set(["a.txt"]));
    expect(bodies.bodyOf("a.txt").patch).toContain("+unkeyed");
    bodies.dispose();
  });

  it("settles a landed body with a newer key until the list changes", async () => {
    let calls = 0;
    const bodies = layer(async () => {
      calls += 1;
      return { files: [{ path: "a.txt", content_key: "newer" }], patch: patchFor("a.txt", "newer") };
    });
    const views = [{ path: "a.txt", contentKey: "old" }];
    const open = new Set(["a.txt"]);
    await bodies.sync(views, open);
    await bodies.sync(views, open);
    expect(calls).toBe(1);

    views[0].contentKey = "another edit";
    await bodies.sync(views, open);
    expect(calls).toBe(2);
    bodies.dispose();
  });

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

  /** A stack with no bodies is a list of headers, so the viewport calls every
   *  file visible. The budget is what stops one turn pulling a whole
   *  changeset: a screenful goes out, the paint that follows makes the stack
   *  taller, and what is now below the fold is never asked for. */
  it("asks for a screenful a turn, not the whole changeset", async () => {
    const asked = [];
    const bodies = layer(async (paths) => {
      asked.push([...paths]);
      return {
        files: paths.map((path) => ({ path, content_key: `k:${path}` })),
        patch: paths.map((path) => patchFor(path, "body")).join(""),
      };
    });
    const views = Array.from({ length: 40 }, (_, index) => ({
      path: `part-${index}.txt`,
      contentKey: `k:part-${index}.txt`,
    }));
    const open = new Set(views.map((view) => view.path));

    const filled = await bodies.sync(views, open);

    expect(filled).toBe(BODIES_PER_TURN);
    expect(asked).toEqual([views.slice(0, BODIES_PER_TURN).map((view) => view.path)]);
    // And the turn after asks for the next screenful, never the same one.
    await bodies.sync(views, open);
    expect(asked[1][0]).toBe(`part-${BODIES_PER_TURN}.txt`);
  });

  it("honours a budget the caller names", async () => {
    const bodies = layer(async (paths) => ({
      files: paths.map((path) => ({ path, content_key: `k:${path}` })),
      patch: paths.map((path) => patchFor(path, "body")).join(""),
    }));
    const views = [
      { path: "a.txt", contentKey: "k:a.txt" },
      { path: "b.txt", contentKey: "k:b.txt" },
    ];

    expect(await bodies.sync(views, new Set(["a.txt", "b.txt"]), { budget: 1 })).toBe(1);
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
