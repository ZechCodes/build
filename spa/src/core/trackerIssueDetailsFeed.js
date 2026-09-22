// Cached issue details for list and Dashboard attention. The list supplies the
// issue ids and their update stamps; the cache supplies the issue and timeline.
// Wire answers only write records. Cache announcements are the sole path by
// which a fetched detail reaches a reader.

import { readCached, readCachedMany, subscribeCache } from "./localCache.js";
import { TRACKER_ISSUE_KIND, issueAddress, issueRecord, writeIssueRecord } from "./trackerCache.js";

const MAX_READS = 4;

const changedSince = (listIssue, detail) => {
  if (!detail?.issue) return true;
  const listedAt = listIssue?.updated_at;
  if (!listedAt) return false;
  const detailedAt = detail.issue.updated_at;
  if (!detailedAt) return true;
  const listTime = Date.parse(listedAt);
  const detailTime = Date.parse(detailedAt);
  return Number.isFinite(listTime) && Number.isFinite(detailTime)
    ? listTime > detailTime
    : listedAt !== detailedAt;
};

/**
 * Follow the details for a project's current issues. `read()` is synchronous
 * and contains only records actually read from IndexedDB. `updateIssues()`
 * resolves after the first cache scan, while any needed live reads continue in
 * the background. Call `dispose()` when the pane unmounts.
 */
export function createTrackerIssueDetailsFeed({ deviceId, projectId, callRpc, onChange = () => {} }) {
  const listed = new Map();
  const cached = new Map(); // id -> { at, value }
  const readEpoch = new Map();
  const pending = new Map(); // id -> list update stamp
  const active = new Set();
  const attempted = new Map(); // one failed or stale response per list version
  let signature = null;
  let generation = 0;
  let readSerial = 0;
  let disposed = false;

  const address = (id) => issueAddress(deviceId, projectId, id);
  const nextEpoch = (id) => {
    const epoch = ++readSerial;
    readEpoch.set(id, epoch);
    return epoch;
  };
  const versionOf = (issue) => String(issue?.updated_at || "");
  const stillWanted = (id, version) => !disposed && listed.has(id) && version === versionOf(listed.get(id));

  function currentRecordWins(current, before, answer) {
    if ((current?.at || 0) > before) return true;
    const heldIssue = current?.value?.issue;
    const answerIssue = answer?.issue;
    return Boolean(heldIssue && answerIssue
      && !changedSince(answerIssue, current.value)
      && heldIssue.updated_at !== answerIssue.updated_at);
  }

  function pump() {
    if (disposed) return;
    for (const [id, version] of pending) {
      if (active.size >= MAX_READS) break;
      if (active.has(id)) continue;
      pending.delete(id);
      if (!stillWanted(id, version)) continue;
      active.add(id);
      attempted.set(id, version);
      void fetchDetail(id, version);
    }
  }

  async function writeAnswer(id, version, answer, before) {
    if (!stillWanted(id, version)) return;
    if (answer?.issue?.id !== id) return;
    // A page or another tab may have written a newer detail while this read
    // was in flight. Leave its record in place rather than replacing it.
    const current = await readCached(address(id));
    if (!stillWanted(id, version) || currentRecordWins(current, before, answer)) return;
    await writeIssueRecord(deviceId, projectId, id, issueRecord(answer?.issue, answer?.timeline));
  }

  async function fetchDetail(id, version) {
    try {
      const before = cached.get(id)?.at || 0;
      const answer = await callRpc("issues.get", { issue_id: id });
      await writeAnswer(id, version, answer, before);
    } catch {
      // The list remains useful offline. A later list version can retry.
    } finally {
      active.delete(id);
      pump();
    }
  }

  function queueIfBehind(id, detail) {
    const issue = listed.get(id);
    if (!changedSince(issue, detail)) {
      pending.delete(id);
      return;
    }
    const version = versionOf(issue);
    if (attempted.get(id) === version) return;
    pending.set(id, version);
    pump();
  }

  function replaceCached(id, row) {
    const previous = cached.get(id);
    if (row?.value) cached.set(id, row);
    else cached.delete(id);
    return previous?.at !== row?.at || Boolean(previous) !== Boolean(row?.value);
  }

  function accept(id, row, epoch) {
    if (disposed || !listed.has(id) || readEpoch.get(id) !== epoch) return false;
    const changed = replaceCached(id, row);
    if (!row?.value) attempted.delete(id);
    queueIfBehind(id, row?.value);
    return changed;
  }

  async function rereadOne(id) {
    if (!listed.has(id) || disposed) return;
    const epoch = nextEpoch(id);
    const row = await readCached(address(id));
    if (accept(id, row, epoch)) onChange();
  }

  const unsubscribe = subscribeCache({ deviceId, entityId: String(projectId || ""), kind: TRACKER_ISSUE_KIND }, (changed) => {
    if (changed.sub && listed.has(changed.sub)) void rereadOne(changed.sub);
    else if (!changed.sub) void scan(); // project eviction may remove every detail
  });

  async function scan() {
    if (disposed) return;
    const current = generation;
    const ids = [...listed.keys()];
    const epochs = ids.map(nextEpoch);
    const rows = await readCachedMany(ids.map(address));
    if (disposed || generation !== current) return;
    let changed = false;
    ids.forEach((id, index) => { if (accept(id, rows[index], epochs[index])) changed = true; });
    if (changed) onChange();
  }

  async function updateIssues(issues) {
    if (disposed) return;
    const next = new Map((issues || []).filter((issue) => issue?.id).map((issue) => [issue.id, issue]));
    const nextSignature = JSON.stringify([...next].map(([id, issue]) => [id, versionOf(issue)]));
    if (signature === nextSignature) return;
    signature = nextSignature;
    generation += 1;
    let removed = false;
    for (const id of listed.keys()) {
      if (next.has(id)) continue;
      cached.delete(id);
      readEpoch.delete(id);
      pending.delete(id);
      attempted.delete(id);
      removed = true;
    }
    listed.clear();
    for (const [id, issue] of next) listed.set(id, issue);
    if (removed) onChange();
    await scan();
  }

  return {
    updateIssues,
    read: () => new Map([...cached].map(([id, row]) => [id, row.value])),
    dispose: () => {
      disposed = true;
      generation += 1;
      pending.clear();
      unsubscribe();
    },
  };
}
