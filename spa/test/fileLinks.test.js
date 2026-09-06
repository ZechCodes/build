// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { renderFilesTab } from "../src/views/files.js";

describe("conversation file-link navigation", () => {
  it("opens the linked file in its containing directory", async () => {
    const calls = [];
    const callRpc = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "fs.tree") {
        return { path: "src", entries: [{ kind: "file", name: "parser.js", size: 12 }] };
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

    renderFilesTab(body, { scope: { run_id: "run-1" }, callRpc, openAt: { path: "src/parser.js" } });
    await vi.waitFor(() => expect(body.querySelector(".fppath")?.textContent).toBe("src/parser.js"));

    expect(calls[0]).toEqual({ method: "fs.tree", params: { run_id: "run-1", path: "src" } });
    expect(calls[1]).toEqual({ method: "fs.read", params: { run_id: "run-1", path: "src/parser.js" } });
    expect(body.querySelector(".ffile").classList.contains("sel")).toBe(true);
  });
});
