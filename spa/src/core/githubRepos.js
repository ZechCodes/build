// The GitHub repositories one machine's `gh` can reach: the bridge's
// github.repos, held in the cache per device and searched here.
//
// The bridge only runs gh and answers what it printed; keeping the list,
// ranking it and deciding what to show are this module's. A sheet that holds a
// Git remote URL paints from the cached record and asks the machine once, in
// the background, when it opens (`refreshGithubRepos`); the answer is written
// to the cache and every picker over that device redraws from it.
//
// The record is `{ repos, refusal }`: the last list the machine answered, or
// `null` when it never has, and the sentence its last refusal said. A refusal
// never takes a list away — a stale list still searches.

import { bridgeCapabilities } from "./changeEvents.js";
import { mergeCached, readCached, subscribeCache } from "./localCache.js";

export const githubReposAddress = (deviceId = "") => ({ deviceId, entityId: "", kind: "github-repos" });

/** How many repositories a search shows at once. */
export const PICKER_ROWS = 8;

/**
 * Ask `deviceId` for its repositories once, through the caller the sheet
 * already holds for that machine, and cache the answer. A bridge that does
 * not announce github.repos is never asked. Nothing is thrown: the cache is
 * where the outcome goes, and the picker paints it.
 */
export async function refreshGithubRepos(deviceId, call) {
  if (!deviceId || !bridgeCapabilities(deviceId).github?.repos) return;
  let next;
  try {
    const answer = await call("github.repos");
    const repos = Array.isArray(answer?.repos) ? answer.repos : [];
    next = () => ({ repos, refusal: "" });
  } catch (error) {
    const refusal = error?.message || String(error);
    next = (held) => ({ repos: Array.isArray(held?.repos) ? held.repos : null, refusal });
  }
  await mergeCached(githubReposAddress(deviceId), next);
}

// ---------------------------------------------------------------- held ---

/** What each device's record last read as, so a picker mounted on a repaint
 *  paints in the same turn instead of after a cache read. */
const held = new Map();
const watchers = new Map();

/** The device's record as last read, or null before the first read lands. */
export const heldGithubRepos = (deviceId) => held.get(deviceId) ?? null;

/** Hear every change to `deviceId`'s record. Answers the way to stop. One cache
 *  subscription serves every picker over the same machine. */
export function watchGithubRepos(deviceId, listener) {
  const watcher = watchers.get(deviceId) || startWatching(deviceId);
  watcher.listeners.add(listener);
  return () => {
    watcher.listeners.delete(listener);
    if (watcher.listeners.size) return;
    watcher.unsubscribe();
    watchers.delete(deviceId);
  };
}

function startWatching(deviceId) {
  const address = githubReposAddress(deviceId);
  const watcher = { listeners: new Set(), unsubscribe: null };
  const reread = async () => {
    const record = await readCached(address);
    held.set(deviceId, record?.value ?? null);
    for (const listener of [...watcher.listeners]) listener();
  };
  watcher.unsubscribe = subscribeCache(address, () => void reread());
  watchers.set(deviceId, watcher);
  void reread();
  return watcher;
}

// ---------------------------------------------------------------- rank ---

const CONSECUTIVE = 1;
const WORD_START = 0.9;
const AFTER_SEPARATOR = 0.6;
const GAP = -0.01;

/** What a match at `index` earns for where it lands: the start of the owner or
 *  the repository, then the start of a word inside either. */
function bonusAt(text, index) {
  if (index === 0 || text[index - 1] === "/") return WORD_START;
  return "-_. ".includes(text[index - 1]) ? AFTER_SEPARATOR : 0;
}

/** One query character against every position of the text: the best score
 *  ending in a match there, and the best score up to there. */
function scoreRow(char, index, text, lower, previous) {
  const match = new Array(text.length).fill(-Infinity);
  const best = new Array(text.length).fill(-Infinity);
  let running = -Infinity;
  for (let at = 0; at < text.length; at += 1) {
    if (lower[at] === char) {
      const fresh = index === 0 ? 0 : (previous.best[at - 1] ?? -Infinity);
      const chained = index === 0 ? -Infinity : (previous.match[at - 1] ?? -Infinity) + CONSECUTIVE;
      match[at] = Math.max(fresh + bonusAt(text, at), chained);
    }
    running = Math.max(match[at], running + GAP);
    best[at] = running;
  }
  return { match, best };
}

/**
 * How well `query` matches `text` as a subsequence, higher is better, or null
 * when it does not. Contiguous runs and hits at the start of a word score
 * highest, so `own/rep` finds `owner/repo` before `someone/owned-prep`.
 */
export function fuzzyScore(query, text) {
  const wanted = query.toLowerCase();
  if (!wanted) return 0;
  if (wanted.length > text.length) return null;
  const lower = text.toLowerCase();
  let row = { match: [], best: [] };
  for (let index = 0; index < wanted.length; index += 1) row = scoreRow(wanted[index], index, text, lower, row);
  // Scored where the last character matched: what trails the match costs
  // nothing, so equal hits fall to the most recent push, not the shortest name.
  const score = Math.max(...row.match);
  return score === -Infinity ? null : score;
}

const pushedAt = (repo) => Date.parse(repo.pushed_at || "") || 0;
const byRecentPush = (a, b) => pushedAt(b.repo) - pushedAt(a.repo);

/** The repositories `query` finds, best first, at most `limit`. An empty query
 *  lists the most recently pushed. */
export function rankRepos(query, repos, limit = PICKER_ROWS) {
  const wanted = query.trim();
  return repos
    .map((repo) => ({ repo, score: fuzzyScore(wanted, repo.name_with_owner || "") }))
    .filter((hit) => hit.score !== null)
    .sort((a, b) => b.score - a.score || byRecentPush(a, b))
    .slice(0, limit)
    .map((hit) => hit.repo);
}
