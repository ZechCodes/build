// @vitest-environment jsdom
// The ⋯ menu's Project settings sheet: what the bridge actually holds for a
// project (name, path, base branch) plus the one project-level mutation it
// exposes for an existing project — project.set_remote.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { openProjectSettings } from "../src/sheets/projectSettings.js";

const PROJECT = {
  project_id: "proj-1",
  name: "build",
  path: "/Users/z/Projects/build",
  base_branch: "main",
  remote: "git@github.com:8ly/build.git",
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

describe("openProjectSettings", () => {
  it("shows the project's identity read-only and its remote as the one editable field", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [PROJECT] });
    openProjectSettings("proj-1", { callRpc });
    await flush();
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector("#psname").value).toBe("build");
    expect(sheet.querySelector("#pspath").value).toBe("/Users/z/Projects/build");
    expect(sheet.querySelector("#psbranch").value).toBe("main");
    expect(sheet.querySelector("#psremote").value).toBe("git@github.com:8ly/build.git");
    expect(sheet.querySelector("#psname").readOnly).toBe(true);
    expect(sheet.querySelector("#pspath").readOnly).toBe(true);
    expect(sheet.querySelector("#psbranch").readOnly).toBe(true);
  });

  it("saves the remote through the bridge's own method and closes", async () => {
    const callRpc = vi.fn().mockImplementation((method) =>
      method === "project.list" ? Promise.resolve({ projects: [PROJECT] }) : Promise.resolve({}),
    );
    openProjectSettings("proj-1", { callRpc });
    await flush();
    document.getElementById("psremote").value = "git@github.com:8ly/other.git";
    document.getElementById("pssave").click();
    await flush();
    expect(callRpc).toHaveBeenCalledWith("project.set_remote", {
      project_id: "proj-1",
      url: "git@github.com:8ly/other.git",
    });
    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
  });

  it("keeps the sheet open and names the failure when the save is rejected", async () => {
    const callRpc = vi.fn().mockImplementation((method) =>
      method === "project.list"
        ? Promise.resolve({ projects: [PROJECT] })
        : Promise.reject(new Error("not a git remote")),
    );
    openProjectSettings("proj-1", { callRpc });
    await flush();
    document.getElementById("pssave").click();
    await flush();
    expect(document.getElementById("pserr").textContent).toContain("not a git remote");
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
  });

  it("reports a project the bridge no longer knows rather than an empty sheet", async () => {
    const callRpc = vi.fn().mockResolvedValue({ projects: [] });
    openProjectSettings("proj-9", { callRpc });
    await flush();
    expect(document.getElementById("sheet").textContent).toContain("no longer");
    expect(document.getElementById("psremote")).toBeNull();
  });

  it("escapes what the project record carries", async () => {
    const callRpc = vi.fn().mockResolvedValue({
      projects: [{ ...PROJECT, name: '"><img src=x>', path: "<b>p</b>" }],
    });
    openProjectSettings("proj-1", { callRpc });
    await flush();
    const sheet = document.getElementById("sheet");
    expect(sheet.querySelector("img")).toBeNull();
    expect(sheet.querySelector("b")).toBeNull();
    expect(sheet.querySelector("#psname").value).toBe('"><img src=x>');
  });
});
