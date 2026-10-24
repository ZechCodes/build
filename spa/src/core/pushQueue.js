// Keep one cache applier running. Pending compatible snapshots share a job;
// thread deltas retain their items and gaps. Board and generation transitions
// divide the queue, so foreground promotion cannot cross a removal or reset.
import { mergeThreadItems } from "./thread.js";
import { isFinishedState } from "./inbox.js";

const tipKey = (tip) => JSON.stringify([tip.agent_id, tip.conversation_id, tip.thread_id, tip.thread_generation_revision]);
const generations = (item) => new Map([
  ...(item.state?.agents || []).map((agent) => [agent.conversation_id || agent.id, agent.thread_id || ""]),
  ...(item.thread || []).map((tip) => [tip.conversation_id || tip.agent_id, tip.thread_id || ""]),
]);
const rosterKey = (item) => item.state && JSON.stringify((item.state.agents || []).map((agent) =>
  [agent.id, agent.conversation_id, agent.thread_id, agent.thread_generation_revision]));
const terminalSource = (item) => item.project_source && item.terminals
  && Object.keys(item).every((key) => ["entity_id", "project_source", "terminals"].includes(key));
const boundary = (item) => item.entity_id === "board" || (item.project_source && !terminalSource(item))
  || isFinishedState(item.state?.state) || item.thread?.some((tip) => tip.reset);
const FOLDABLE = new Set(["entity_id", "state", "thread", "terminals", "tasks", "files"]);

const sameTerminalSource = (left, right) => Boolean(terminalSource(left) && terminalSource(right))
  && JSON.stringify(left.project_source) === JSON.stringify(right.project_source);

function compatible(left, right) {
  if (left.project_source || right.project_source) return sameTerminalSource(left, right);
  if (boundary(left) || boundary(right)) return false;
  if (![...Object.keys(left), ...Object.keys(right)].every((key) => FOLDABLE.has(key))) return false;
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
  const merged = { ...left, ...right };
  if (left.thread || right.thread) merged.thread = combineTips(left.thread, right.thread);
  if (left.files || right.files) merged.files = combineInvalidation(left.files, right.files, "paths");
  if (left.tasks || right.tasks) merged.tasks = combineInvalidation(left.tasks, right.tasks, "task_ids");
  return merged;
}

export const combinePushItems = (left, right) => compatible(left, right) ? combineItems(left, right) : null;

export function createPushQueue({ apply, priority = () => false, onError = () => {}, merge = combinePushItems, isBarrier = boundary }) {
  const segments = [];
  let running = 0, done = null, promoted = 0;

  function append(context, item) {
    if (isBarrier(item)) { const job = { context, item }; segments.push({ barrier: job }); return job; }
    let segment = segments.at(-1);
    if (!segment || segment.barrier) { segment = { entities: new Map() }; segments.push(segment); }
    const key = item.project_source ? JSON.stringify(item.project_source) : String(item.entity_id || "");
    const jobs = segment.entities.get(key) || [];
    const last = jobs.at(-1);
    const merged = last?.context.active() && merge(last.item, item);
    if (merged) { last.item = merged; last.context = context; }
    else jobs.push({ context, item });
    segment.entities.set(key, jobs);
    return jobs.at(-1);
  }

  function take() {
    const segment = segments[0];
    if (segment.barrier) { segments.shift(); return segment.barrier; }
    const entries = [...segment.entities];
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

  async function drain() {
    running = 1;
    // Publish the shared drain promise before even an empty/inactive run ends.
    await Promise.resolve();
    try {
      while (segments.length) {
        const job = take();
        try {
          if (job.context.active()) await apply(job.context, job.item);
        } catch (error) { onError(error); }
        finally { job.resolve?.(); }
      }
    } finally { running = 0; done = null; }
  }

  function schedule() {
    if (!segments.length) return done || Promise.resolve();
    if (!done) done = drain();
    return done;
  }

  return {
    enqueue(context, items) {
      for (const item of items) append(context, item);
      return schedule();
    },
    enqueueApplied(context, items) {
      const completions = items.map((item) => {
        const job = append(context, item);
        job.done ||= new Promise((resolve) => { job.resolve = resolve; });
        return job.done;
      });
      void schedule();
      return Promise.all(completions);
    },
    snapshot: () => ({ running, pending: segments.reduce((count, segment) => count + (segment.barrier ? 1
      : [...segment.entities.values()].reduce((sum, jobs) => sum + jobs.length, 0)), 0) }),
    clear: () => {
      for (const segment of segments) {
        const jobs = segment.barrier ? [segment.barrier] : [...segment.entities.values()].flat();
        for (const job of jobs) job.resolve?.();
      }
      segments.length = 0;
    },
  };
}
