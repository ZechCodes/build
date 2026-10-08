// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { retireFileUploads, uploadsFor } from "../src/core/fileUploads.js";
import { renderFilesTab } from "../src/views/files.js";

let files, uploads;
afterEach(async () => {
  files?.dispose();
  // This standalone fixture has no cache scope, so its upload owner is undefined.
  retireFileUploads(undefined);
  await uploads?.ready;
  files = null;
  uploads = null;
});

describe("conversation file-link navigation", () => {
  it("opens the linked file with the tree expanded down to it", async () => {
    const calls = [];
    const callRpc = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "fs.tree") {
        return params.path === "src"
          ? { path: "src", entries: [{ kind: "file", name: "parser.js", size: 12 }] }
          : { path: "", entries: [{ kind: "dir", name: "src" }] };
      }
      return {
        path: "src/parser.js",
        size: 12,
        truncated: false,
        mime: "text/plain",
        content_b64: btoa("const ok = 1"),
      };
    });
    const body = document.createElement("div");

    files = renderFilesTab(body, { scope: { run_id: "run-1" }, callRpc, openAt: { path: "src/parser.js" } });
    uploads = uploadsFor(undefined);
    await vi.waitFor(() => expect(body.querySelector(".fppath")?.textContent).toBe("src/parser.js"));

    expect(calls).toContainEqual({ method: "fs.tree", params: { run_id: "run-1", path: "" } });
    expect(calls).toContainEqual({ method: "fs.read", params: { run_id: "run-1", path: "src/parser.js" } });
    // The tree opens down to the file, and its row is the highlighted one.
    await vi.waitFor(() => expect(body.querySelector(".ffile.sel")?.dataset.path).toBe("src/parser.js"));
    expect(calls).toContainEqual({ method: "fs.tree", params: { run_id: "run-1", path: "src" } });
  });
});
