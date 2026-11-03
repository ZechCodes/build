// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTaskReviewFeedback, openTaskReviewer } from "../src/core/taskReviewFeedback.js";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { timelineHtml } from "../src/core/trackerTaskRender.js";


globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const anchor = { snapshot_id: "snap-1", directory_id: "dir:1", path: "a<&.js", side: "new", line: 12 };
const until = async (ready) => {
  for (let attempt = 0; attempt < 40 && !ready(); attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  expect(ready()).toBe(true);
};
const mount = (host, callRpc = vi.fn(async () => ({})), over = {}) => mountTaskReviewFeedback(host, {
  deviceId: "device", projectId: "project", taskId: "task", snapshot: { id: "snap-1", number: 2 },
  callRpc, ...over,
});

beforeEach(() => { document.body.innerHTML = ""; });

describe("task review feedback", () => {
  it("renders old anchor metadata and a reply action after a review is replaced", () => {
    const rows = timelineRows([{ type: "comment", id: "tc-2", body: "old", author: { kind: "user" },
      anchor, opinion: { snapshot_id: "snap-1", verdict: "approve" }, reply_to: "tc-1" }]);
    const html = timelineHtml(rows, { reviewSnapshots: [{ id: "snap-1", number: 2 }] });
    expect(html).toContain("Revision 2");
    expect(html).not.toContain("Revision snap-1");
    expect(html).toContain("Approved");
    expect(html).toContain("data-review-anchor=");
    expect(html).toContain("data-review-reply=\"tc-2\"");
    expect(html).toContain("a&lt;&amp;.js");
    const withoutMetadata = timelineHtml(rows, { reviewSnapshots: [{ id: "new-snapshot", number: 3 }] });
    expect(withoutMetadata).toContain("Earlier revision");
    expect(withoutMetadata).not.toContain("Revision snap-1");
  });

  it("opens the ordinary assignee picker with the snapshot in its note", async () => {
    const picker = openTaskReviewer({
      task: { id: "task", number: 7, title: "Check API", assignee: null },
      snapshot: { id: "snap-1", number: 2 },
      feed: { items: [], workspaces: [] }, projectKey: "device|project",
      callRpc: vi.fn(),
    });
    expect(document.querySelector("#task-assign-note").value).toContain("revision 2 of task #7");
    await picker.close();
  });

  it("collects inline comments in a tray, edits, jumps and removes them", async () => {
    const host = document.createElement("div");
    const verdictHost = document.createElement("div");
    const inline = document.createElement("div");
    document.body.append(host, verdictHost, inline);
    const onJump = vi.fn();
    const feedback = mount(host, vi.fn(), { taskId: "tray", verdictHost, onJump, placeComposer: (form) => { inline.append(form); return true; } });
    await feedback.ready;
    await feedback.comment(anchor);
    const form = inline.querySelector('[data-review-inline-composer]');
    expect(form).not.toBeNull();
    const field = form.querySelector("textarea");
    field.value = "Review this line";
    field.dispatchEvent(new Event("input"));
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await until(() => host.querySelectorAll('[data-review-draft]').length === 1);
    expect(host.textContent).toContain("Review this line");
    host.querySelector('[data-review-draft-jump]').click();
    expect(onJump).toHaveBeenCalledWith(anchor, "changes");
    host.querySelector('[data-review-draft-edit]').click();
    await until(() => inline.querySelector('textarea')?.value === "Review this line");
    const edit = inline.querySelector('textarea');
    edit.value = "Edited"; edit.dispatchEvent(new Event("input"));
    await until(() => host.textContent.includes("Edited"));
    host.querySelector('[data-review-draft-remove]').click();
    await until(() => host.querySelectorAll('[data-review-draft]').length === 0);
    feedback.dispose();
  });

  it("restores pending comments after remount and keeps historical verdicts disabled", async () => {
    const host = document.createElement('div');
    const bar = document.createElement('div');
    const inline = document.createElement('div');
    document.body.append(host, bar, inline);
    const options = { taskId: "reload", verdictHost: bar, latestSnapshotId: () => "newer", placeComposer: (form) => { inline.append(form); return true; } };
    const first = mount(host, vi.fn(), options);
    await first.ready;
    await first.comment(anchor);
    const field = inline.querySelector('textarea');
    field.value = 'Retained'; field.dispatchEvent(new Event('input'));
    await until(() => host.textContent.includes('Retained'));
    first.dispose();
    const next = mount(host, vi.fn(), options);
    await next.ready;
    expect(host.textContent).toContain('Retained');
    expect(host.textContent).toContain('earlier revision');
    expect([...bar.querySelectorAll('[data-review-verdict]')].every((button) => button.disabled)).toBe(true);
    expect(bar.querySelectorAll('[data-review-verdict]').length).toBe(2);
    expect(host.querySelector('select')).toBeNull();
    next.dispose();
  });

  it("holds a refused batch in the tray and restores drafts for correction", async () => {
    const host = document.createElement('div'); const bar = document.createElement('div'); const inline = document.createElement('div');
    document.body.append(host, bar, inline);
    const rows = new Map(); let change;
    const service = { subscribe: (fn) => { change = fn; return () => {}; }, readOperations: () => [...rows.values()],
      enqueueBatch: vi.fn(async (inputs) => { inputs.forEach((row) => rows.set(row.operationId, { ...row, state: 'queued', acknowledged: false })); return inputs.map((row) => row.operationId); }),
      cancelUnsentBatch: vi.fn(async () => {}) };
    const feedback = mount(host, vi.fn(), { taskId: 'refused', writeOperations: service, verdictHost: bar, placeComposer: (form) => { inline.append(form); return true; } });
    await feedback.ready; await feedback.comment(anchor);
    const field = inline.querySelector('textarea'); field.value = 'Fix this'; field.dispatchEvent(new Event('input'));
    await until(() => host.querySelectorAll('[data-review-draft]').length === 1);
    inline.querySelector('form').dispatchEvent(new Event('submit', { cancelable: true }));
    host.querySelector('[data-review-comment-batch]').click();
    await until(() => rows.size === 1);
    const row = [...rows.values()][0];
    rows.set(row.operationId, { ...row, acknowledged: true, state: 'failed', effect: 'unapplied', error: { message: 'That line is unavailable' } }); change();
    await until(() => host.textContent.includes('That line is unavailable'));
    expect(host.querySelector('[data-review-draft-edit]').disabled).toBe(true);
    host.querySelector('[data-review-recover]').click();
    await until(() => !host.querySelector('[data-review-draft-edit]').disabled);
    host.querySelector('[data-review-draft-remove]').click();
    await until(() => host.querySelectorAll('[data-review-draft]').length === 0);
    feedback.dispose();
  });

  it("offers retry when durable enqueue fails without discarding the saved review", async () => {
    const host = document.createElement('div'); const bar = document.createElement('div'); const inline = document.createElement('div');
    document.body.append(host, bar, inline);
    const enqueueBatch = vi.fn().mockRejectedValueOnce(new Error('Local storage is full')).mockResolvedValue([]);
    const service = { subscribe: () => () => {}, readOperations: () => [], enqueueBatch };
    const feedback = mount(host, vi.fn(), { taskId: 'enqueue-error', writeOperations: service, verdictHost: bar, placeComposer: (form) => { inline.append(form); return true; } });
    await feedback.ready; await feedback.comment(anchor);
    const field = inline.querySelector('textarea'); field.value = 'Keep this draft'; field.dispatchEvent(new Event('input'));
    await until(() => host.querySelectorAll('[data-review-draft]').length === 1);
    inline.querySelector('form').dispatchEvent(new Event('submit', { cancelable: true }));
    host.querySelector('[data-review-comment-batch]').click();
    await until(() => host.textContent.includes('Local storage is full'));
    expect(host.querySelector('[data-review-retry]').hidden).toBe(false);
    expect(host.querySelector('[data-review-recover]').hidden).toBe(true);
    host.querySelector('[data-review-retry]').click();
    await until(() => enqueueBatch.mock.calls.length === 2);
    expect(enqueueBatch.mock.calls[1][0]).toEqual(enqueueBatch.mock.calls[0][0]);
    expect(host.textContent).toContain('Keep this draft');
    feedback.dispose();
  });

  it.each(['approve', 'request_changes'])("confirms %s with what happens next and submits pending comments", async (verdict) => {
    const host = document.createElement('div');
    const bar = document.createElement('div');
    document.body.append(host, bar);
    const callRpc = vi.fn(async () => ({}));
    const feedback = mount(host, callRpc, { taskId: verdict, verdictHost: bar });
    await feedback.ready;
    bar.querySelector(`[data-review-verdict="${verdict}"]`).click();
    await until(() => Boolean(document.querySelector('#confirm-scrim')));
    expect(document.querySelector('#confirm-scrim').textContent).toContain('The author is told and continues');
    document.querySelector('[data-confirm-ok]').click();
    await until(() => callRpc.mock.calls.length === 1);
    expect(callRpc.mock.calls[0][1]).toMatchObject({ opinion: { snapshot_id: 'snap-1', verdict } });
    feedback.dispose();
  });
});
