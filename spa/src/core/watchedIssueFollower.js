// The inbox's watched issues (#125), followed through the cache.
//
// Each project the rail lists has a cached `issues.list` record, which the
// sync layer re-reads on every pass and on every `issues` push
// (core/cacheSync.js). This follows those records, and keeps a cached
// `issues.get` record for every watched issue that is still open — the
// timeline is what says whether an agent has said something new — reading
// one only when the list says the issue has moved since. A read's answer
// only writes the cache; the rail repaints on the cache's announcement.
//
// Every project the rail lists is followed, whether or not its machine has
// greeted this tab yet: what makes a row is the cached records, and a cold
// reload paints them before any machine answers. The watch is proof enough of
// the bridge — only one that carries watching ever marks an issue `watched`,
// and only watched issues are rows or have their details read.

import { subscribeCache } from "./localCache.js";
import { contextFor } from "./deviceContexts.js";
import { isFinished } from "./trackerAgentIssues.js";
import { createTrackerIssueDetailsFeed } from "./trackerIssueDetailsFeed.js";
import { issuesAddress, readIssuesRecord } from "./trackerCache.js";
import { needsYouRuleAddress, readNeedsYouRule } from "./needsYouRule.js";
import { watchedIssueEntries } from "./watchedIssueRows.js";
import { issueUnreadTally } from "./issueUnread.js";

const followsDetail = (issue) => issue?.watched === true && !isFinished(issue);

/** Asks the machine for whatever session it is on when the read is made. */
const callOn = (deviceId) => (method, params) => {
  const context = contextFor(deviceId);
  return context ? context.rpc(method, params) : Promise.reject(new Error("This machine is not connected."));
};

/** One project's list and the details of its open watched issues, and the
 *  Needs you rule its machine's issues are read by (core/needsYouRule.js). */
function followProject(project, onChange) {
  const { deviceId, id: projectId } = project;
  let issues = [];
  let askedOnly = false;
  let reads = 0;
  let disposed = false;
  const details = createTrackerIssueDetailsFeed({ deviceId, projectId, callRpc: callOn(deviceId), onChange });

  async function reread() {
    const read = ++reads;
    const [record, rule] = await Promise.all([readIssuesRecord(deviceId, projectId), readNeedsYouRule(deviceId)]);
    if (disposed || read !== reads) return;
    issues = record?.issues || [];
    askedOnly = rule;
    onChange();
    await details.updateIssues(issues.filter(followsDetail));
  }

  const unsubscribe = subscribeCache(issuesAddress(deviceId, projectId), () => void reread());
  const unsubscribeRule = subscribeCache(needsYouRuleAddress(deviceId), () => void reread());
  void reread();
  return {
    // A reconnect is a new session: reads the old one refused are asked again.
    session: contextFor(deviceId)?.session || null,
    source: () => ({ project, issues, details: details.read(), askedOnly }),
    dispose() {
      disposed = true;
      unsubscribe();
      unsubscribeRule();
      details.dispose();
    },
  };
}

const projectKeyOf = (project) => `${project.deviceId}|${project.id}`;

/**
 * Follow the watched issues of `projects` as the rail lists them. `onChange`
 * is called whenever a followed record changes; `entries()` answers the rows
 * as the cache holds them now, and `issueUnread()` the watched issues' unread
 * (core/issueUnread.js). Call `follow(projects)` whenever the rail's
 * projects or a machine's connection change.
 */
export function followWatchedIssues({ onChange = () => {} } = {}) {
  const followed = new Map();

  function drop(key) {
    followed.get(key)?.dispose();
    followed.delete(key);
  }

  function follow(projects = []) {
    const wanted = new Map(projects.map((project) => [projectKeyOf(project), project]));
    for (const key of followed.keys()) if (!wanted.has(key)) drop(key);
    for (const [key, project] of wanted) {
      const standing = followed.get(key);
      if (standing && standing.session === (contextFor(project.deviceId)?.session || null)) continue;
      drop(key);
      followed.set(key, followProject(project, onChange));
    }
  }

  const sources = () => [...followed.values()].map((one) => one.source());

  return {
    follow,
    entries: () => watchedIssueEntries(sources()),
    /** Where the rail wears each watched issue's unread (#104). */
    issueUnread: () => issueUnreadTally(sources()),
    dispose: () => [...followed.keys()].forEach(drop),
  };
}
