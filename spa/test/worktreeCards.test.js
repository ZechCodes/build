import { describe, it, expect } from "vitest";
import { externalWorktreeCard } from "../src/core/worktreeCards.js";

const base = {
  worktree_id: "wt-3fa9c04d21ab",
  project_id: "proj-1",
  project: "Build",
  path: "/Users/z/Build-hotfix",
  branch: "hotfix/login",
  head_subject: "Fix login redirect",
  head_age_seconds: 7200,
  dirty_files: 3,
  diffstat: { files_changed: 5, insertions: 120, deletions: 8 },
};

describe("externalWorktreeCard", () => {
  it("carries the worktree and project ids on the card dataset", () => {
    const html = externalWorktreeCard(base);
    expect(html).toContain('data-wt="wt-3fa9c04d21ab"');
    expect(html).toContain('data-project="proj-1"');
    expect(html).toContain('class="card quiet external"');
  });

  it("renders branch, subject, diffstat, dirty count, and age", () => {
    const html = externalWorktreeCard(base);
    expect(html).toContain("hotfix/login");
    expect(html).toContain("Fix login redirect");
    expect(html).toContain("5 files +120 −8");
    expect(html).toContain("3 uncommitted");
    expect(html).toContain("2h ago");
    expect(html).toContain("WORKTREE");
  });

  it("shows (detached) for a null branch and omits the dirty suffix when clean", () => {
    const html = externalWorktreeCard({ ...base, branch: null, dirty_files: 0 });
    expect(html).toContain("(detached)");
    expect(html).not.toContain("uncommitted");
  });

  it("tolerates a missing diffstat", () => {
    const html = externalWorktreeCard({ ...base, diffstat: undefined });
    expect(html).toContain("0 files +0 −0");
  });

  it("escapes untrusted branch names and commit subjects", () => {
    const html = externalWorktreeCard({
      ...base,
      branch: "<img src=x onerror=alert(1)>",
      head_subject: "<script>alert(1)</script>",
    });
    expect(html).not.toContain("<img src=x");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).toContain("&lt;script&gt;");
  });
});
