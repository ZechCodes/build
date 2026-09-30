// What a Git source's card says about keeping its base branch in step with
// its remote (#267). Read only off the source row the cache holds: the
// bridge's service writes what each sync concluded onto the row, and the
// sheet repaints when the row does.

import { humanAge } from "./text.js";

const plural = (count, word) => `${count} ${word}${count === 1 ? "" : "s"}`;

const ageOf = (ms, now) => humanAge((now - ms) / 1000);

const counts = ({ ahead, behind }) => {
  const parts = [ahead ? `${ahead} ahead` : "", behind ? `${behind} behind` : ""].filter(Boolean);
  return parts.length ? ` (${parts.join(", ")})` : "";
};

function syncedLine(source, sync, now) {
  const moved = sync.commits
    ? `moved ${source.base_branch} forward ${plural(sync.commits, "commit")}`
    : `up to date${sync.ahead ? `, ${sync.ahead} ahead` : ""}`;
  return `Synced ${ageOf(sync.last_attempt_ms, now)} · ${moved}.`;
}

const skippedLine = (_source, sync, now) =>
  `Skipped ${ageOf(sync.last_attempt_ms, now)}${counts(sync)}: ${sync.reason}`;

function failedLine(_source, sync, now) {
  const since = sync.last_synced_ms ? ` Last synced ${ageOf(sync.last_synced_ms, now)}.` : "";
  const waits = sync.needs_you ? " Build will not try again on its own until you press Sync now." : "";
  return `Last sync failed ${ageOf(sync.last_attempt_ms, now)}: ${sync.reason}${since}${waits}`;
}

const LINES = {
  synced: syncedLine,
  skipped: skippedLine,
  failed: failedLine,
  no_remote: () => "No remote to sync with.",
};

/** One sentence: whether the base is kept up to date, and how the last sync
 *  of it went. */
export function syncStatusLine(source, now = Date.now()) {
  if (!source.sync_base) return `Off. New workspaces start from ${source.base_branch} as it stands.`;
  const sync = source.sync;
  const line = sync && LINES[sync.state];
  return line ? line(source, sync, now) : "Not synced yet.";
}

const attemptOf = (source) => source?.sync?.last_attempt_ms ?? -1;

/** `project` with each source's `sync` taken from `listed` (the same project
 *  as the pushed project list has it) where that one is newer, or null when
 *  none is. Only the sync moves: the rest of the row is the sheet's own. */
export function withNewerSyncs(project, listed) {
  const listedById = new Map((listed?.sources || []).map((source) => [source.id, source]));
  const newer = (source) => attemptOf(listedById.get(source.id)) > attemptOf(source);
  const sources = project.sources || [];
  if (!sources.some(newer)) return null;
  return {
    ...project,
    sources: sources.map((source) => (newer(source) ? { ...source, sync: listedById.get(source.id).sync } : source)),
  };
}
