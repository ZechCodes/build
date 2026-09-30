// What a source card says about keeping its base branch in step with its
// remote (#267), read only off the cached row.

import { describe, expect, it } from "vitest";
import { syncStatusLine, withNewerSyncs } from "../src/core/sourceSyncModel.js";

const NOW = 1_800_000_000_000;
const MIN = 60_000;

const source = (sync, extra = {}) => ({ id: "source-1", is_git: true, base_branch: "main", sync_base: true, sync, ...extra });
const status = (fields) => ({
  state: "synced", ahead: 0, behind: 0, commits: 0, needs_you: false,
  last_attempt_ms: NOW - 3 * MIN, last_synced_ms: NOW - 3 * MIN, last_fetched_ms: NOW - 3 * MIN, ...fields,
});

describe("syncStatusLine", () => {
  it("says a source that is off is cut from its base as it stands", () => {
    expect(syncStatusLine(source(null, { sync_base: false }), NOW))
      .toBe("Off. New workspaces start from main as it stands.");
  });

  it("says a source not synced yet is waiting for its first sync", () => {
    expect(syncStatusLine(source(null), NOW)).toBe("Not synced yet.");
  });

  it("says when a level base was last synced", () => {
    expect(syncStatusLine(source(status({})), NOW)).toBe("Synced 3m ago · up to date.");
  });

  it("says how far a sync moved the base", () => {
    expect(syncStatusLine(source(status({ commits: 1 })), NOW)).toBe("Synced 3m ago · moved main forward 1 commit.");
    expect(syncStatusLine(source(status({ commits: 4 })), NOW)).toBe("Synced 3m ago · moved main forward 4 commits.");
  });

  it("says a base ahead of its remote is ahead", () => {
    expect(syncStatusLine(source(status({ ahead: 2 })), NOW)).toBe("Synced 3m ago · up to date, 2 ahead.");
  });

  it("gives the reason and the counts for a skip", () => {
    const skipped = status({ state: "skipped", reason: "main has 1 commit origin does not.", ahead: 1, behind: 4 });
    expect(syncStatusLine(source(skipped), NOW))
      .toBe("Skipped 3m ago (1 ahead, 4 behind): main has 1 commit origin does not.");
  });

  it("gives the reason for a failure and when it last worked", () => {
    const failed = status({ state: "failed", reason: "origin did not answer.", last_attempt_ms: NOW - 2 * MIN, last_synced_ms: NOW - 120 * MIN });
    expect(syncStatusLine(source(failed), NOW)).toBe("Last sync failed 2m ago: origin did not answer. Last synced 2h ago.");
  });

  it("says a remote that needed you waits for Sync now", () => {
    const failed = status({ state: "failed", reason: "origin did not answer.", needs_you: true, last_synced_ms: null });
    expect(syncStatusLine(source(failed), NOW))
      .toBe("Last sync failed 3m ago: origin did not answer. Build will not try again on its own until you press Sync now.");
  });

  it("says a checkout with no remote has nothing to sync with", () => {
    expect(syncStatusLine(source(status({ state: "no_remote" })), NOW)).toBe("No remote to sync with.");
  });
});

describe("withNewerSyncs", () => {
  const project = { project_id: "proj-1", sources: [source(status({ last_attempt_ms: 10 })), { id: "source-2", sync_base: false, sync: null }] };

  it("takes a newer sync from the pushed list, and only the sync", () => {
    const listed = { sources: [{ ...source(status({ last_attempt_ms: 20, commits: 3 })), sync_base: false, name: "renamed" }] };
    const taken = withNewerSyncs(project, listed);
    expect(taken.sources[0].sync.commits).toBe(3);
    expect(taken.sources[0].sync_base).toBe(true);
    expect(taken.sources[0].name).toBeUndefined();
    expect(taken.sources[1]).toBe(project.sources[1]);
  });

  it("keeps what the sheet holds when the list is no newer, or carries no sources", () => {
    expect(withNewerSyncs(project, { sources: [source(status({ last_attempt_ms: 10 }))] })).toBeNull();
    expect(withNewerSyncs(project, { sources: [source(status({ last_attempt_ms: 5 }))] })).toBeNull();
    expect(withNewerSyncs(project, { project_id: "proj-1" })).toBeNull();
    expect(withNewerSyncs(project, null)).toBeNull();
  });
});
