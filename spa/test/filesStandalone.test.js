// @vitest-environment jsdom
// The Files tab is one of the two tabs a branch now has, so it must stand on
// its own: a host element, a scope, and an RPC channel — no tab shell, no
// cluster, no surrounding view. This mounts it exactly that way and walks it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { renderFilesTab } from "../src/views/files.js";

const base64 = (text) => Buffer.from(text, "utf8").toString("base64");

const TREE = {
  "": { path: "", entries: [{ name: "src", kind: "dir" }, { name: "README.md", kind: "file", size: 12 }] },
  src: { path: "src", entries: [{ name: "a.js", kind: "file", size: 20 }] },
};

const FILES = {
  "README.md": { mime: "text/markdown", size: 12, truncated: false, content_b64: base64("# Title\n") },
  "src/a.js": { mime: "text/plain", size: 20, truncated: false, content_b64: base64("const a = 1;\n") },
};

function mountFiles({ scope = { project_id: "p1", worktree_id: "w1" }, openAt = null } = {}) {
  const calls = [];
  const host = document.createElement("div");
  document.body.appendChild(host);
  const files = renderFilesTab(host, {
    scope,
    openAt,
    callRpc: async (method, params) => {
      calls.push({ method, params });
      if (method === "fs.tree") return TREE[params.path || ""];
      if (method === "fs.read") return FILES[params.path];
      throw new Error(`unexpected ${method}`);
    },
  });
  return { host, files, calls };
}

describe("the Files browser on its own", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("lists the scope root and previews a file, carrying the scope on every call", async () => {
    const { host, files, calls } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    expect(host.querySelectorAll(".frow")).toHaveLength(2);

    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".fpbody")).toBeTruthy());
    expect(host.querySelector(".fppath").textContent).toBe("README.md");
    expect(host.querySelector(".fpbody").textContent).toContain("Title");
    // Every fs call is scope-spread: the browser never sends a host path.
    expect(calls.every((call) => call.params.project_id === "p1" && call.params.worktree_id === "w1")).toBe(true);
    files.dispose();
  });

  it("walks into a directory and back out again", async () => {
    const { host, files } = mountFiles();
    await vi.waitFor(() => expect(host.querySelector(".fdir")).toBeTruthy());
    host.querySelector(".fdir").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("src"));
    expect(host.querySelector(".ffile").textContent).toContain("a.js");

    host.querySelector(".fup").click();
    await vi.waitFor(() => expect(host.querySelector(".fcrumb").textContent).toBe("/"));
    files.dispose();
  });

  it("opens the file a deep link named, without anything else pointing it there", async () => {
    const { host, files } = mountFiles({ openAt: { path: "src/a.js" } });
    await vi.waitFor(() => expect(host.querySelector(".fppath")).toBeTruthy());
    expect(host.querySelector(".fppath").textContent).toBe("src/a.js");
    expect(host.querySelector(".fcrumb").textContent).toBe("src");
    files.dispose();
  });

  it("says so, and stays usable, when a file cannot be read", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const files = renderFilesTab(host, {
      scope: { run_id: "r1" },
      callRpc: async (method) => {
        if (method === "fs.tree") return TREE[""];
        throw new Error("permission denied");
      },
    });
    await vi.waitFor(() => expect(host.querySelector(".ffile")).toBeTruthy());
    host.querySelector(".ffile").click();
    await vi.waitFor(() => expect(host.querySelector(".fpidle").textContent).toContain("permission denied"));
    expect(host.querySelector(".ftree").querySelectorAll(".frow")).toHaveLength(2);
    files.dispose();
  });
});
