/** @vitest-environment jsdom */
import { describe, expect, it, vi } from "vitest";
import { mountTaskCommentRecovery } from "../src/core/taskCommentRecovery.js";

describe("cached task comment failures", () => {
  it("revisits draft restoration after an acknowledgement finishes clearing the sent composer", async () => {
    document.body.innerHTML = '<main><form data-task-composer></form></main>';
    const row = { operationId: "fast-failure", state: "failed", effect: "unapplied", draft: { body: "keep this" }, owner: { kind: "task-comment", taskId: "task", projectId: "project" } };
    const service = { readOperations: () => [row], subscribe: () => () => {} };
    const restoreDraft = vi.fn().mockResolvedValueOnce(false).mockResolvedValue(true);
    const recovery = mountTaskCommentRecovery({ host: document.querySelector("main"), service, taskId: "task", projectId: "project", restoreDraft, retry: vi.fn() });
    recovery.sync();
    await new Promise((resolve) => setTimeout(resolve, 0));
    recovery.sync();
    expect(restoreDraft).toHaveBeenCalledTimes(2);
    await new Promise((resolve) => setTimeout(resolve, 0));
    recovery.sync();
    expect(restoreDraft).toHaveBeenCalledTimes(2);
    recovery.dispose();
  });

  it("restores an unapplied draft, retries a fresh user action, checks unknown outcomes under the same ID and dismisses", async () => {
    document.body.innerHTML = '<main><form data-task-composer></form></main>';
    const listeners = new Set();
    let rows = [];
    const service = { readOperations: () => rows, subscribe: (fn) => { listeners.add(fn); return () => listeners.delete(fn); }, checkAgain: vi.fn(async () => {}), dismiss: vi.fn(async () => {}) };
    const retry = vi.fn(async () => {}), restoreDraft = vi.fn();
    const recovery = mountTaskCommentRecovery({ host: document.querySelector("main"), service, taskId: "task", projectId: "project", retry, restoreDraft });
    const row = { operationId: "accepted-comment", state: "failed", effect: "unapplied", error: { message: "Disk refused the comment" }, draft: { body: "my comment" }, owner: { kind: "task-comment", taskId: "task", projectId: "project" } };
    rows = [row]; listeners.forEach((fn) => fn());
    expect(restoreDraft).toHaveBeenCalledWith(row.draft);
    expect(document.body.textContent).toContain("Your comment wasn’t saved");
    document.querySelector('[data-write-action="retry"]').click();
    await vi.waitFor(() => expect(retry).toHaveBeenCalledWith(row));
    await vi.waitFor(() => expect(service.dismiss).toHaveBeenCalledWith(row.operationId));
    rows = [{ ...row, effect: "unknown" }]; listeners.forEach((fn) => fn());
    expect(document.querySelector('[data-write-action="retry"]').textContent).toBe("Check again");
    document.querySelector('[data-write-action="retry"]').click();
    await vi.waitFor(() => expect(service.checkAgain).toHaveBeenCalledWith(row.operationId));
    expect(retry).toHaveBeenCalledTimes(1);
    document.querySelector('[data-write-action="dismiss"]').click();
    await vi.waitFor(() => expect(service.dismiss).toHaveBeenCalledTimes(2));
    recovery.dispose();
    expect(listeners.size).toBe(0);
  });
});
