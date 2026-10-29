// A mounted task page admits one fixed, bounded set per adopted content turn.
// Index repaints never rescan or refill it. Only the point reader writes cache;
// markdown continues resolving synchronously through the cache-only index.
import { referencesIn } from "./markdownRefs.js";
import { admitTaskReferenceLookups } from "./taskReferenceDemand.js";
import { createTaskLookupReader } from "./taskAggregateReads.js";
import { contextFor, deviceContextIdentity, whenGreeted } from "./deviceContexts.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import { onBridgeGreeted } from "./changeEvents.js";
import { taskMutationEpoch, taskMutationsPending, subscribeTaskMutations } from "./taskMutationFence.js";
import { trailingRead } from "./trailingRead.js";

const MAX_TEXT_NODES = 256;
const MAX_TEXT_BYTES = 16_384;
const MAX_NODE_CHARS = 4096;
const encoder = new TextEncoder();

const taskNumbersIn = (text) => referencesIn(text).filter((reference) => reference.kind === "task"
  && Number.isSafeInteger(reference.number) && reference.number > 0).map((reference) => reference.number);

function* contentParts(content) {
  yield content.body || "";
  for (const row of (content.rows || []).slice(0, MAX_TEXT_NODES)) if (row.type === "comment") yield row.body;
}

function boundedContentKey(content) {
  if (typeof content === "string") return content;
  const parts = [];
  let bytes = 0;
  for (const part of contentParts(content)) {
    if (parts.length === MAX_TEXT_NODES) break;
    const text = String(part || "").slice(0, MAX_NODE_CHARS);
    bytes += encoder.encode(text).length;
    if (bytes > MAX_TEXT_BYTES) break;
    parts.push(text);
  }
  return JSON.stringify(parts);
}

function admittedNumbers(host) {
  const numbers = new Set();
  let nodes = 0, bytes = 0;
  for (const body of host.querySelectorAll(".task-page-body.markdown,.task-comment-body.markdown")) {
    const walker = host.ownerDocument.createTreeWalker(body, 4); // SHOW_TEXT
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (++nodes > MAX_TEXT_NODES) return [...numbers];
      const text = node.data.slice(0, MAX_NODE_CHARS);
      bytes += encoder.encode(text).length;
      if (bytes > MAX_TEXT_BYTES) return [...numbers];
      if (node.parentElement?.closest("code,pre,a")) continue;
      for (const number of taskNumbersIn(text)) {
        numbers.add(number);
        if (numbers.size === 4) return [...numbers];
      }
    }
  }
  return [...numbers];
}

function readAuthority(deviceId, context) {
  let held;
  return () => {
    const session = deviceSession(deviceId), greeting = context?.greeted, epoch = taskMutationEpoch(deviceId);
    if (!held || held.session !== session || held.greeting !== greeting || held.epoch !== epoch) held = { session, greeting, epoch };
    return held;
  };
}

export function mountTaskReferenceLookups(host, { deviceId, projectId, context = contextFor(deviceId) }) {
  const identity = deviceContextIdentity(deviceId), watch = deviceWatch(deviceId);
  const authority = readAuthority(deviceId, context);
  let disposed = false, current = null;
  const ownsScope = () => !disposed && contextFor(deviceId) === context
    && deviceContextIdentity(deviceId) === identity && context?.active?.() === true;
  const canRead = () => ownsScope() && !watch.away() && Boolean(deviceSession(deviceId))
    && taskMutationsPending(deviceId) === 0;

  async function readTurn(turn, force) {
    if (!canRead() || current !== turn) return false;
    const issued = authority();
    const request = await whenGreeted(context, (stands) => {
      if (!canRead() || current !== turn || authority() !== issued) return false;
      turn.stands = stands;
      return Promise.all(turn.numbers.map(async (number) => {
        const cached = await turn.reader.read(number);
        if (!force && cached.status !== "unknown") return false;
        return turn.reader.refresh(number);
      }));
    });
    return request ? request.sent : false;
  }

  function stopTurn() { current?.reader.dispose(); current?.release(); }

  function update(content) {
    const key = boundedContentKey(content);
    if (disposed || current?.key === key) return Promise.resolve(false);
    stopTurn();
    const turn = { key, numbers: [], release: () => {}, stands: () => false };
    current = turn;
    turn.reader = createTaskLookupReader({
      scope: { deviceId, projectId }, callRpc: (...args) => context.rpc(...args),
      capabilities: () => context?.adapter?.capabilities?.tasks,
      sessionOf: authority, active: () => canRead() && current === turn && turn.stands(),
      mutationEpoch: () => taskMutationEpoch(deviceId),
    });
    turn.run = trailingRead((force) => readTurn(turn, force), { generationOf: authority });
    // take() adopts cache content before its synchronous paint. Scan afterwards,
    // once, with the content token captured before any point request can finish.
    return Promise.resolve().then(() => {
      if (disposed || current !== turn) return false;
      turn.numbers = admittedNumbers(host);
      turn.release = admitTaskReferenceLookups({ deviceId, projectId }, turn.numbers);
      return turn.run(false);
    }).catch(() => false);
  }

  let scheduled = null;
  function refresh() {
    if (disposed || !current || !canRead()) return Promise.resolve(false);
    if (!scheduled) scheduled = Promise.resolve().then(() => {
      scheduled = null;
      return current?.run(true);
    }).catch(() => false);
    return scheduled;
  }

  const stopGreeting = onBridgeGreeted((id) => { if (id === deviceId) void refresh(); });
  let lastAuthority = authority(), lastAway = watch.away();
  const stopMoved = watch.moved(() => {
    const next = authority(), away = watch.away();
    if (lastAuthority === next && lastAway === away) return;
    lastAuthority = next; lastAway = away;
    if (!away) void refresh();
  });
  const stopMutations = subscribeTaskMutations(deviceId, ({ phase, pending }) => {
    if (phase === "settled" && pending === 0) void refresh();
  });
  return { update, refresh, dispose() {
    disposed = true; stopTurn(); stopGreeting(); stopMoved(); stopMutations();
  } };
}
