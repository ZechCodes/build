// Independent tasks.filterChoices membership. Wire pages only write the cache;
// readers paint through read(), and continuation is always explicitly requested.
import { mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { nextTaskRead } from "./taskReadOrder.js";

export const TASK_FILTER_CHOICES_KIND = "tracker-filter-choices";
const DIMENSIONS = new Set(["label", "assignee", "state", "status"]);

export function filterChoicesAddress({ deviceId, projectId }, dimension) {
  if (!DIMENSIONS.has(dimension)) throw new Error("Invalid task filter choice dimension");
  return { deviceId, entityId: String(projectId), kind: TASK_FILTER_CHOICES_KIND, sub: dimension };
}

export async function readFilterChoices(scope, dimension) {
  return (await readCached(filterChoicesAddress(scope, dimension)))?.value || null;
}

/** Reserve a cross-reader generation while retaining the last successful paint.
 * Acceptance is checked inside the transaction, including account/session life. */
export async function beginFilterChoices(scope, dimension, generation, { accept = () => true } = {}) {
  if (!Number.isSafeInteger(generation) || generation <= 0) throw new Error("Invalid filter choices generation");
  const address = filterChoicesAddress(scope, dimension);
  const written = await mergeCachedRecordsTogether([address], ([record]) => {
    const held = record?.value;
    if (!accept() || held?.generation >= generation) return null;
    return [{ choices: [], next_cursor: null, complete: false, ...held, generation, refreshing: true }];
  });
  return written ? { scope: { ...scope }, dimension, address, generation, cursor: null } : null;
}

function choiceKey(dimension, value) {
  const selections = canonicalSummaryFilters({ [dimension]: value });
  const canonical = Object.values(selections)[0]?.[0];
  if (canonical == null) throw new Error("Invalid task filter choice");
  return JSON.stringify(canonical);
}

function mergeChoices(dimension, previous, incoming) {
  const values = new Map();
  for (const value of [...previous, ...incoming]) {
    const key = choiceKey(dimension, value);
    if (!values.has(key)) values.set(key, value);
  }
  return [...values.values()];
}

function validatePage(ticket, page) {
  if (page?.project_id !== ticket.scope.projectId || page.dimension !== ticket.dimension
    || !Array.isArray(page.choices)) throw new Error("Expected matching task filter choices page");
  const next = page.next_cursor;
  if (next != null && (typeof next !== "string" || !next || next === ticket.cursor)) {
    throw new Error("Invalid filter choices continuation");
  }
}

function acceptsPage(held, ticket, accept) {
  if (!accept() || held?.generation !== ticket.generation) return false;
  if (ticket.cursor === null) return held.refreshing === true;
  return !held.refreshing && !held.complete && held.next_cursor === ticket.cursor;
}

/** Cursor and deduplicated choices commit together. An empty page may still
 * advance continuation; stale, duplicate and rejected pages consume nothing. */
export function landFilterChoices(ticket, page, { accept = () => true } = {}) {
  validatePage(ticket, page);
  const incoming = mergeChoices(ticket.dimension, [], page.choices);
  return mergeCachedRecordsTogether([ticket.address], ([record]) => {
    const held = record?.value;
    if (!acceptsPage(held, ticket, accept)) return null;
    return [{ ...held,
      choices: mergeChoices(ticket.dimension, ticket.cursor === null ? [] : held.choices, incoming),
      next_cursor: page.next_cursor ?? null, complete: page.next_cursor == null, refreshing: false,
    }];
  });
}

function retainFailedRefresh(ticket, accept) {
  return mergeCachedRecordsTogether([ticket.address], ([record]) => {
    const held = record?.value;
    if (!acceptsPage(held, ticket, accept)) return null;
    return [{ ...held, refreshing: false }];
  });
}

const canContinue = (held, ticket) => held && ticket && held.generation === ticket.generation
  && !held.refreshing && !held.complete && typeof held.next_cursor === "string";

/** Injectable authority mirrors the summary reader. Summary capability never
 * implies choice support; cached choices stay readable without either one. */
export function createTaskFilterChoicesReader({ scope, dimension, callRpc, capabilities, sessionOf, active = () => true }) {
  filterChoicesAddress(scope, dimension);
  let disposed = false;
  let serial = 0;
  let ticket = null;
  let ticketSession = null;
  let pending = null;
  const read = () => readFilterChoices(scope, dimension);
  const available = () => !disposed && active() && capabilities()?.filterChoices === true;
  const accepts = (generation, session) => available() && generation === serial && sessionOf() === session;

  async function ask(next, generation, session) {
    if (!accepts(generation, session)) return false;
    const accept = () => accepts(generation, session);
    try {
      const page = await callRpc("tasks.filterChoices", { project_id: scope.projectId, dimension, limit: 100,
        ...(next.cursor ? { cursor: next.cursor } : {}) });
      return await landFilterChoices(next, page, { accept });
    } catch (error) {
      if (next.cursor === null) await retainFailedRefresh(next, accept);
      throw error;
    }
  }

  function track(operation) {
    const promise = operation();
    pending = promise;
    const settle = () => { if (pending === promise) pending = null; };
    void promise.then(settle, settle);
    return promise;
  }

  function refresh() {
    if (!available()) return Promise.resolve(false);
    const generation = ++serial;
    const session = sessionOf();
    ticketSession = session;
    return track(async () => {
      const order = await nextTaskRead();
      const next = await beginFilterChoices(scope, dimension, order, { accept: () => accepts(generation, session) });
      if (!next || !accepts(generation, session)) return false;
      ticket = next;
      return ask(next, generation, session);
    });
  }

  async function continuePage() {
    const generation = serial;
    const session = ticketSession;
    const next = ticket;
    const held = await read();
    if (!accepts(generation, session) || !canContinue(held, next)) return false;
    return ask({ ...next, cursor: held.next_cursor }, generation, session);
  }

  function more() {
    if (!available() || !ticket) return Promise.resolve(false);
    return pending || track(continuePage);
  }

  return { read, refresh, more, dispose() { disposed = true; serial += 1; } };
}
