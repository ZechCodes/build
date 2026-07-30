// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/app.js";
import { openImplementOptions } from "../src/sheets/implement.js";

describe("Implement All options", () => {
  beforeEach(() => {
    document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  });

  it("preserves Implement All semantics while applying overrides", async () => {
    const call = vi.fn().mockResolvedValue({ issue_id: "issue-1" });
    App.call = call;
    const result = openImplementOptions(
      {
        issue_id: "issue-1",
        plan_id: "issue-1",
        goal: "Ship it",
        base_branch: "main",
        provider: "claude",
      },
      {},
    );
    document.querySelector("#implbase").value = "release";
    document.querySelector("#implstart").click();
    await expect(result).resolves.toEqual({ issue_id: "issue-1" });
    expect(call).toHaveBeenCalledWith(
      "issue.implement_all",
      expect.objectContaining({ issue_id: "issue-1", base_branch: "release" }),
    );
    expect(document.querySelector("#sheet").textContent).not.toContain("Create a run");
  });
});
