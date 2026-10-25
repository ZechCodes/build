// Keep one cache applier running. Pending compatible snapshots share a job;
// thread deltas retain their items and gaps. Destructive board and generation
// transitions divide the queue, so foreground promotion cannot cross a removal
// or reset.
import { mergeThreadItems } from "./thread.js";
import { isFinishedState } from "./inbox.js";

const tipKey = (tip) => JSON.stringify([tip.agent_id, tip.conversation_id, tip.thread_id, tip.thread_generation_revision]);
const generations = (item) => new Map([
  ...(item.state?.agents || []).map((agent) => [agent.conversation_id || agent.id, agent.thread_id || ""]),
  ...(item.thread || []).map((tip) => [tip.conversation_id || tip.agent_id, tip.thread_id || ""]),
]);
const rosterKey = (item) => item.state && JSON.stringify([item.state.kind, (item.state.agents || []).map((agent) =>
  [agent.id, agent.conversation_id, agent.thread_id, agent.thread_generation_revision])]);
const terminalSource = (item) => item.project_source && item.terminals
  && Object.keys(item).every((key) => ["entity_id", "project_source", "terminals"].includes(key));
// A numeric revision is a dirty/high-water hint, not an ownership transition.
// Unknown board payloads stay ordered until their causal semantics are known.
const boardRevisionHint = (item) => item.entity_id === "board"
  && Object.keys(item).every((key) => ["entity_id", "state"].includes(key))
  && Number.isFinite(item.state?.revision) && Object.keys(item.state).length === 1;
const boundary = (item) => (item.entity_id === "board" && !boardRevisionHint(item)) || (item.project_source && !terminalSource(item))
  || isFinishedState(item.state?.state) || item.thread?.some((tip) => tip.reset);
const FOLDABLE = new Set(["entity_id", "state", "thread", "terminals", "tasks", "files"]);
const stateCanFold = (state) => !state || typeof state.kind === "string" && state.kind !== ""
  && Array.isArray(state.agents || []);
const itemCanFold = (item) => !boundary(item) && stateCanFold(item.state)
  && Object.keys(item).every((key) => FOLDABLE.has(key));

const sameTerminalSource = (left, right) => Boolean(terminalSource(left) && terminalSource(right))
  && JSON.stringify(left.project_source) === JSON.stringify(right.project_source);

function compatible(left, right) {
  if (boardRevisionHint(left) && boardRevisionHint(right)) return true;
  if (left.project_source || right.project_source) return sameTerminalSource(left, right);
  if (!itemCanFold(left) || !itemCanFold(right)) return false;
  if (left.state && right.state && rosterKey(left) !== rosterKey(right)) return false;
  const held = generations(left);
  return [...generations(right)].every(([id, generation]) => !held.has(id) || held.get(id) === generation);
}

function combineTip(left, right) {
  const latest = Number(right.last_sequence || 0) >= Number(left.last_sequence || 0) ? right : left;
  return { ...latest, since_sequence: Math.min(Number(left.since_sequence || 0), Number(right.since_sequence || 0)),
    last_sequence: Math.max(Number(left.last_sequence || 0), Number(right.last_sequence || 0)),
    items: mergeThreadItems(left.items || [], right.items || []) };
}

const carriesCoverage = (tip) => tip.items?.length || Number(tip.last_sequence || 0) <= Number(tip.since_sequence || 0);
const sameCoverageClass = (left, right) => Boolean(carriesCoverage(left)) === Boolean(carriesCoverage(right));
const overlappingTips = (left, right) => Number(right.since_sequence || 0) <= Number(left.last_sequence || 0)
  && Number(left.since_sequence || 0) <= Number(right.last_sequence || 0);

function combineTips(left = [], right = []) {
  const tips = [...left];
  for (const tip of right) {
    const index = tips.findLastIndex((held) => tipKey(held) === tipKey(tip));
    const held = tips[index];
    if (held && sameCoverageClass(held, tip) && overlappingTips(held, tip)) tips[index] = combineTip(held, tip);
    else tips.push(tip);
  }
  return tips;
}

const union = (left, right) => [...new Set([...(left || []), ...(right || [])])];
const combineInvalidation = (left, right, field) => !left || !right ? right || left
  : { ...left, ...right, [field]: union(left[field], right[field]), truncated: Boolean(left.truncated || right.truncated) };

function combineItems(left, right) {
  if (boardRevisionHint(left) && boardRevisionHint(right)) {
    return { ...right, state: { revision: Math.max(left.state.revision, right.state.revision) } };
  }
  const merged = { ...left, ...right };
  if (left.thread || right.thread) merged.thread = combineTips(left.thread, right.thread);
  if (left.files || right.files) merged.files = combineInvalidation(left.files, right.files, "paths");
  if (left.tasks || right.tasks) merged.tasks = combineInvalidation(left.tasks, right.tasks, "task_ids");
  return merged;
}

export const combinePushItems = (left, right) => compatible(left, right) ? combineItems(left, right) : null;

export function createPushQueue({ apply, priority = () => false, onError = () => {}, merge = combinePushItems,
  isBarrier = boundary, concurrency = 1, onChange = () => {} }) {
  const segments = [], busy = new Set(), trackedJobs = new Set();
  let running = 0, done = null, resolveIdle, promoted = 0, barrierRunning = false;
  const keyOf = (item) => item.project_source ? JSON.stringify(item.project_source) : String(item.entity_id || "");

  function newJob(context, item, barrier = false) {
    const job = { context, item, barrier };
    trackedJobs.add(job);
    return job;
  }

  function completion(job) {
    job.done ||= new Promise((resolve) => { job.resolve = resolve; });
    return job.done;
  }

  function append(context, item) {
    if (isBarrier(item)) { const job = newJob(context, item, true); segments.push({ barrier: job }); return job; }
    let segment = segments.at(-1);
    if (!segment || segment.barrier) { segment = { entities: new Map() }; segments.push(segment); }
    const key = keyOf(item);
    const jobs = segment.entities.get(key) || [];
    const last = jobs.at(-1);
    const merged = last && !last.captured && last.context.active() && merge(last.item, item);
    if (merged) { last.item = merged; last.context = context; }
    else jobs.push(newJob(context, item));
    segment.entities.set(key, jobs);
    return jobs.at(-1);
  }

  function take() {
    const segment = segments[0];
    if (segment.barrier) return running ? null : segments.shift().barrier;
    const entries = [...segment.entities].filter(([key]) => !busy.has(key));
    if (!entries.length) return null;
    const preferred = entries.find(([, pending]) => priority(pending[0].item));
    const selected = promoted < 4 && preferred ? preferred : entries[0];
    promoted = selected === preferred ? promoted + 1 : 0;
    const [key, jobs] = selected;
    const job = jobs.shift();
    segment.entities.delete(key);
    if (jobs.length) segment.entities.set(key, jobs);
    if (!segment.entities.size) segments.shift();
    return job;
  }

  async function run(job) {
    let result;
    try {
      if (job.context.active()) result = await apply(job.context, job.item, () => Boolean(job.captured));
    } catch (error) { onError(error); }
    finally {
      job.resolve?.(result);
      trackedJobs.delete(job);
      busy.delete(keyOf(job.item));
      if (job.barrier) barrierRunning = false;
      running -= 1;
      pump();
    }
  }

  function pump() {
    while (segments.length && running < concurrency && !barrierRunning) {
      const job = take();
      if (!job) break;
      running += 1;
      barrierRunning = Boolean(job.barrier);
      busy.add(keyOf(job.item));
      void Promise.resolve().then(() => run(job));
    }
    if (!segments.length && !running && done) {
      const resolve = resolveIdle;
      done = null;
      resolve();
    }
    onChange();
  }

  function schedule() {
    if (!segments.length) return done || Promise.resolve();
    done ||= new Promise((resolve) => { resolveIdle = resolve; });
    const pending = done;
    pump();
    return pending;
  }

  return {
    enqueue(context, items) {
      for (const item of items) append(context, item);
      return schedule();
    },
    enqueueApplied(context, items, captured = false) {
      const completions = items.map((item) => {
        const job = append(context, item);
        job.captured ||= captured;
        return completion(job);
      });
      void schedule();
      return Promise.all(completions);
    },
    // Preserve the boundary: later compatible payloads coalesce in a new job.
    capture: () => Promise.all([...trackedJobs].map((job) => {
      job.captured = true;
      return completion(job).then((result) => result?.settled);
    })),
    snapshot: () => ({ running, pending: segments.reduce((count, segment) => count + (segment.barrier ? 1
      : [...segment.entities.values()].reduce((sum, jobs) => sum + jobs.length, 0)), 0) }),
    clear: () => {
      for (const segment of segments) {
        const pending = segment.barrier ? [segment.barrier] : [...segment.entities.values()].flat();
        for (const job of pending) { job.resolve?.(); trackedJobs.delete(job); }
      }
      segments.length = 0;
      pump();
    },
  };
}
