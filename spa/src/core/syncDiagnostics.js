// In-memory timing only: callers supply fixed labels and numeric aggregates,
// never entity addresses, request params, errors, paths or reply bodies.
import { SQL_READ_METHODS } from "./sqlReadCredits.js";

const STAGES = new Set(["greeting", "subscriptions", "held", "lists", "selection", "foreground", "rosters", "inbox", "background", "cleanup", "route"]);
const VERBS = new Set([
  ...SQL_READ_METHODS, "session.hello",
  "board.list", "project.list", "workspace.list", "agent.list", "tasks.list", "tasks.columns",
  "term.list", "git.status", "git.log", "git.show", "git.unpushed", "fs.read", "fs.tree",
  "run.diff", "worktree.diff", "project.agents", "changes.subscribe", "changes.unsubscribe",
  "thread.list", "thread.get", "thread.page", "thread.read", "conversation.get", "conversation.list",
  "messages.list", "messages.get", "tasks.review.list", "tasks.review.get", "tasks.review.diff",
]);
const KINDS = new Set(["rpc", "board", "projects", "workspaces", "agents", "tasks", "tasks-first", "tasks-rest",
  "terminals", "status", "log", "unpushed", "tree", "file", "patch", "diff", "thread", "reviews", "row", "feed"]);
const SUCCESSOR_REASONS = new Set(["mutation", "read-state", "visibility", "connection", "subscription-inbox", "subscription-background"]);
const OUTCOMES = new Set(["reply", "path-timeout", "answer-timeout", "admission-cancel"]);
export const SYNC_DIAGNOSTIC_ITEM_LIMIT = 32;
const QUEUE_COUNTS = ["bodyRunning", "bodyPending", "backgroundRunning", "backgroundPending", "catchupRunning", "catchupPending", "captured", "drained"];
const SELECTED_COUNTS = ["active", "recent", "projectOwner"];
const SQL_COUNTS = ["queued", "admitted", "timedOutHeld"];
const ITEM_COUNTS = ["queued", "settled", "unsettled", "droppedDetail"];
const count = (value) => Number.isFinite(value) ? Math.max(0, Math.min(Number.MAX_SAFE_INTEGER, Math.floor(value))) : 0;
const counters = (source, keys) => Object.fromEntries(keys.map((key) => [key, count(source?.[key])]));
const offset = (now, period) => Math.max(0, now - period.epoch);
const age = (item, at) => (item.settlement ?? at) - item.queued;
const oldest = (items, now) => items.reduce((age, item) => Math.max(age ?? 0, now - item.epoch), null);

function createPeriod(epoch) {
  // Tokens may outlive their period. Shared counters contain no back-reference
  // to the history, so an overflow token cannot keep that history alive.
  const tally = { active: true, ...counters(null, ITEM_COUNTS) };
  return { epoch, tally, stop: null, passCount: 0, passes: [], slowest: [], active: new Set() };
}

function reportPeriod(period, at) {
  const now = offset(at, period);
  const active = [...period.active];
  const slowest = [...period.slowest, ...active.map((item) => item.record)]
    .sort((a, b) => age(b, now) - age(a, now)).slice(0, 5);
  return { start: 0, stop: period.stop, passCount: period.passCount, passes: period.passes, slowest,
    items: { ...counters(period.tally, ITEM_COUNTS), retained: active.length },
    oldestUnsettledMs: oldest(active, at), oldestUnsettledSampled: period.tally.unsettled > active.length };
}

function itemTotals(periods) {
  const total = { ...counters(null, ITEM_COUNTS), retained: 0 };
  for (const period of periods) {
    for (const key of ITEM_COUNTS) total[key] += period.tally[key];
    total.retained += period.active.size;
  }
  return total;
}

function sqlSnapshot(providers) {
  const total = counters(null, SQL_COUNTS);
  for (const provider of providers) {
    const snapshot = counters(provider(), SQL_COUNTS);
    for (const key of SQL_COUNTS) total[key] = count(total[key] + snapshot[key]);
  }
  return total;
}

export function createSyncDiagnostics({ now = () => performance.now() } = {}) {
  let periods = [], current = null, devices = [];
  let passes = new WeakMap(), requests = new WeakMap();
  const unsettled = new Set(), providers = new Set();
  const requestState = Symbol("sync timing counters");
  let lastTime = 0;
  const time = () => {
    const value = now();
    if (Number.isFinite(value)) lastTime = Math.max(lastTime, value);
    return lastTime;
  };

  function dropPeriod(period) {
    period.tally.active = false;
    for (const item of period.active) {
      requests.delete(item.token);
      unsettled.delete(item);
    }
    period.active.clear();
  }

  function ring(active) {
    if (Boolean(active) === Boolean(current)) return;
    if (!active) {
      current.stop = offset(time(), current);
      current = null;
      return;
    }
    current = createPeriod(time());
    periods.push(current);
    for (const period of periods.slice(0, -8)) dropPeriod(period);
    periods = periods.slice(-8);
  }

  function passStart() {
    if (!current) return null;
    const token = {};
    const record = { start: offset(time(), current), end: null, stages: [],
      successorRequests: {}, snapshot: { capture: null, drain: null }, selected: { ...counters(null, SELECTED_COUNTS), heldBodies: null } };
    passes.set(token, { period: current, record });
    current.passCount += 1;
    current.passes.push(record);
    current.passes = current.passes.slice(-32);
    return token;
  }

  function passEnd(token) {
    const pass = passes.get(token);
    if (pass && pass.record.end === null) pass.record.end = offset(time(), pass.period);
  }

  function stageStart(token, stage) {
    const pass = passes.get(token);
    if (!pass || !STAGES.has(stage) || pass.record.stages.length >= 64) return () => {};
    const record = { stage, start: offset(time(), pass.period), end: null };
    pass.record.stages.push(record);
    // clear() invalidates the captured token as well as any future writes.
    return () => {
      if (passes.has(token) && record.end === null) record.end = offset(time(), pass.period);
    };
  }

  function snapshot(token, phase, values) {
    const pass = passes.get(token);
    if (!pass || !["capture", "drain"].includes(phase)) return;
    pass.record.snapshot[phase] = { at: offset(time(), pass.period), ...counters(values, QUEUE_COUNTS) };
  }

  function selected(token, values) {
    const pass = passes.get(token);
    if (!pass) return;
    for (const key of SELECTED_COUNTS) {
      if (Object.hasOwn(values || {}, key)) pass.record.selected[key] = count(values[key]);
    }
  }

  function successor(token, reason) {
    const pass = passes.get(token);
    if (!pass || !SUCCESSOR_REASONS.has(reason)) return;
    const counts = pass.record.successorRequests;
    counts[reason] = count((counts[reason] || 0) + 1);
  }

  function itemQueued(verb, kind) {
    if (!current) return null;
    const tally = current.tally;
    tally.queued += 1;
    tally.unsettled += 1;
    const token = { [requestState]: { tally, settled: false } };
    if (current.active.size >= SYNC_DIAGNOSTIC_ITEM_LIMIT) {
      tally.droppedDetail += 1;
      return token;
    }
    const epoch = time();
    const record = { verb: VERBS.has(verb) ? verb : "unknown", kind: KINDS.has(kind) ? kind : "unknown",
      queued: offset(epoch, current), start: null, receipt: null, settlement: null, outcome: null };
    const item = { token, epoch, period: current, record };
    requests.set(token, item);
    unsettled.add(item);
    current.active.add(item);
    return token;
  }

  function milestone(token, key) {
    const item = requests.get(token);
    if (item && item.record[key] === null) item.record[key] = offset(time(), item.period);
  }

  function itemSettled(token, outcome) {
    const state = token?.[requestState];
    if (!state?.tally.active || state.settled) return;
    state.settled = true;
    state.tally.settled += 1;
    state.tally.unsettled -= 1;
    const item = requests.get(token);
    if (!item) return;
    const { period, record } = item;
    record.settlement = offset(time(), period);
    record.outcome = OUTCOMES.has(outcome) ? outcome : null;
    requests.delete(token);
    unsettled.delete(item);
    period.active.delete(item);
    period.slowest.push(record);
    period.slowest.sort((a, b) => age(b, record.settlement) - age(a, record.settlement));
    period.slowest = period.slowest.slice(0, 5);
  }

  function deviceState(states) {
    devices = states.map((state) => ({ syncing: state.syncing === true, pass: state.pass === true, ...counters(state, QUEUE_COUNTS) }));
  }

  function registerSqlCredits(provider) {
    providers.add(provider);
    return () => providers.delete(provider);
  }

  function report() {
    const at = time();
    const items = itemTotals(periods);
    return structuredClone({ periods: periods.map((period) => reportPeriod(period, at)), devices,
      liveCount: devices.length, sql: sqlSnapshot(providers), items, oldestUnsettledMs: oldest([...unsettled], at),
      oldestUnsettledSampled: items.unsettled > items.retained });
  }

  function clear() {
    const active = Boolean(current);
    for (const period of periods) dropPeriod(period);
    periods = [];
    current = null;
    passes = new WeakMap();
    requests = new WeakMap();
    unsettled.clear();
    if (active) ring(true);
  }

  return { ring, passStart, passEnd, stageStart, snapshot, selected, successor, itemQueued,
    itemStart: (token) => milestone(token, "start"), itemReceipt: (token) => milestone(token, "receipt"),
    itemSettled, deviceState, registerSqlCredits, report, clear };
}

export const syncDiagnostics = createSyncDiagnostics();
