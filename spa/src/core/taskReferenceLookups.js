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

const MAX_DOM_NODES = 256;
const MAX_SOURCE_ROWS = 256;
const MAX_DEPTH = 32;
const MAX_TEXT_BYTES = 16_384;
const MAX_PART_CHARS = 4096;
const encoder = new TextEncoder();
const protectedTags = new Set(["CODE", "PRE", "A"]);
const taskNumbersIn = (text) => referencesIn(text).filter((reference) => reference.kind === "task"
  && Number.isSafeInteger(reference.number) && reference.number > 0).map((reference) => reference.number);

function* contentParts(content) {
  yield { kind: "body", text: content.body || "" };
  for (const row of (content.rows || []).slice(0, MAX_SOURCE_ROWS)) {
    if (row.type === "comment") yield { kind: "comment", id: row.key, text: row.body };
  }
}

function commentIdSize(id) {
  if (typeof id !== "string" || id.length > 128 || !id.isWellFormed()) return null;
  // These distinct raw IDs can normalize to the same HTML attribute. Leave
  // both sides unadmitted rather than infer a canonical identity from the DOM.
  if (/[\0\r\n\ufffd]/u.test(id)) return null;
  const size = encoder.encode(id).length;
  return size > 0 && size <= 128 ? size : null;
}

function sourcePartSize(part) {
  if (typeof part.text !== "string" || part.text.length > MAX_PART_CHARS) return null;
  const idSize = part.kind === "comment" ? commentIdSize(part.id) : 0;
  return idSize === null ? null : idSize + encoder.encode(part.text).length;
}

// Admit only complete bounded source parts. Truncating a part for its identity
// while scanning beyond that prefix in its rendered DOM would miss retargets.
function sourceAdmission(content) {
  const parts = [], comments = new Set();
  let bytes = 0, body = false;
  for (const part of contentParts(content)) {
    const size = sourcePartSize(part);
    if (size === null || bytes + size > MAX_TEXT_BYTES) continue;
    bytes += size;
    parts.push([part.kind, part.id, part.text]);
    if (part.kind === "body") body = true;
    else comments.add(part.id);
  }
  return { key: JSON.stringify(parts), body, comments };
}

function scopeAt(node, parent, admission) {
  const commentId = node.getAttribute("data-comment-id") ?? parent.commentId;
  let inBody = parent.inBody;
  if (node.classList.contains("markdown")) {
    if (node.classList.contains("task-page-body")) inBody = admission.body;
    if (node.classList.contains("task-comment-body")) inBody = commentId?.length <= 128 && admission.comments.has(commentId);
  }
  return { inBody, commentId };
}

function nextFrame(frame, stack, context) {
  const { node, depth } = frame;
  if (node.nextSibling) stack.push({ ...frame, node: node.nextSibling });
  if (node.firstChild && depth < MAX_DEPTH && !protectedTags.has(node.tagName)) {
    return { node: node.firstChild, depth: depth + 1, ...context };
  }
  return stack.pop();
}

function collectNumbers(node, numbers, budget) {
  const text = node.data.slice(0, MAX_PART_CHARS);
  budget.bytes += encoder.encode(text).length;
  if (budget.bytes > MAX_TEXT_BYTES) return;
  for (const number of taskNumbersIn(text)) {
    numbers.add(number);
    if (numbers.size === 4) return;
  }
}

function admittedNumbers(host, admission) {
  const numbers = new Set(), stack = [], budget = { bytes: 0 };
  let frame = { node: host.firstChild, depth: 0, inBody: false, commentId: null };
  for (let visited = 0; frame?.node && visited < MAX_DOM_NODES; visited += 1) {
    const { node } = frame;
    const context = node.nodeType === 1 ? scopeAt(node, frame, admission) : frame;
    if (node.nodeType === 3 && frame.inBody) collectNumbers(node, numbers, budget);
    if (numbers.size === 4 || budget.bytes > MAX_TEXT_BYTES) break;
    frame = nextFrame(frame, stack, context);
  }
  return [...numbers];
}

async function readPoints(turn, force) {
  const settled = await Promise.allSettled(turn.numbers.map(async (number) => {
    const cached = await turn.reader.read(number);
    if (!force && cached.status !== "unknown") return false;
    return turn.reader.refresh(number);
  }));
  // A rejected sibling must not release the trailing turn while other
  // requests from this same authority are still outstanding.
  const failure = settled.find((result) => result.status === "rejected");
  if (failure) throw failure.reason;
  return settled.map((result) => result.value);
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
      return readPoints(turn, force);
    });
    return request ? request.sent : false;
  }

  function stopTurn() { current?.reader.dispose(); current?.release(); }

  function update(content) {
    if (disposed) return Promise.resolve(false);
    const admission = sourceAdmission(content), key = admission.key;
    if (current?.key === key) return Promise.resolve(false);
    stopTurn();
    const turn = { key, numbers: [], release: () => {}, stands: () => false };
    current = turn;
    turn.reader = createTaskLookupReader({
      scope: { deviceId, projectId }, callRpc: (...args) => context.rpc(...args),
      capabilities: () => context?.adapter?.capabilities?.tasks,
      sessionOf: authority, active: () => canRead() && current === turn && turn.stands(),
      mutationEpoch: () => taskMutationEpoch(deviceId),
    });
    turn.force = false;
    turn.run = trailingRead(() => {
      const force = turn.force;
      turn.force = false;
      return readTurn(turn, force);
    }, { generationOf: authority });
    // take() adopts cache content before its synchronous paint. Scan afterwards,
    // once, with the content token captured before any point request can finish.
    return Promise.resolve().then(() => {
      if (disposed || current !== turn) return false;
      turn.numbers = admittedNumbers(host, admission);
      turn.release = admitTaskReferenceLookups({ deviceId, projectId }, turn.numbers);
      return turn.run();
    }).catch(() => false);
  }

  let scheduled = null;
  function refresh() {
    if (disposed || !current || !canRead()) return Promise.resolve(false);
    if (!scheduled) scheduled = Promise.resolve().then(() => {
      scheduled = null;
      if (!current) return false;
      current.force = true;
      return current.run();
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
