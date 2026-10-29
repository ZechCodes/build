// One supported summary query. This controller writes cache records; callers
// paint through read(), never through a returned wire payload. It is not yet
// wired into legacy surfaces while their backend prerequisites are pending.
import { beginSummaryQuery, landSummaryPage, readSummaryQuery } from "./taskSummaryCache.js";
import { canonicalSummaryFilters, summaryFiltersSupported } from "./taskSummaryQuery.js";
import { nextTaskRead } from "./taskReadOrder.js";

export function createTaskSummaryReader({ scope, filters, callRpc, capabilities, sessionOf, active = () => true,
  mutationEpoch = () => 0 }) {
  const params = canonicalSummaryFilters(filters);
  let disposed = false;
  let serial = 0;
  let ticket = null;
  let ticketSession = null;
  let pending = null;
  let refused = false;
  let ticketEpoch;
  const read = () => readSummaryQuery(scope, params);
  const available = () => !disposed && active() && summaryFiltersSupported(filters, capabilities());
  const accepts = (generation, session) => available() && generation === serial && sessionOf() === session
    && mutationEpoch() === ticketEpoch;

  async function ask(next, readOrder, generation, session) {
    if (!accepts(generation, session)) return false;
    try {
      const answer = await callRpc("tasks.list", { project_id: scope.projectId, summary: true, ...params,
        limit: 100, ...(next.cursor ? { cursor: next.cursor } : {}) });
      return await landSummaryPage(next, answer, { read: readOrder, accept: () => accepts(generation, session) });
    } catch (error) {
      if (accepts(generation, session) && stopsContinuation(error)) refused = true;
      throw error;
    }
  }

  function track(operation) {
    const promise = operation();
    pending = promise;
    void promise.then(() => { if (pending === promise) pending = null; }, () => { if (pending === promise) pending = null; });
    return promise;
  }

  function refresh() {
    if (!available()) return Promise.resolve(false);
    const generation = ++serial;
    const session = sessionOf();
    ticketEpoch = mutationEpoch();
    refused = false;
    ticketSession = session;
    return track(async () => {
      const readOrder = await nextTaskRead();
      if (!accepts(generation, session)) return false;
      const next = await beginSummaryQuery(scope, params, readOrder, { accept: () => accepts(generation, session) });
      if (!accepts(generation, session)) return false;
      ticket = next;
      return ask(next, readOrder, generation, session);
    });
  }

  async function continuePage() {
    const generation = serial;
    const session = ticketSession;
    const next = ticket;
    const held = await read();
    if (!accepts(generation, session) || !canContinue(held, next)) return false;
    const readOrder = await nextTaskRead();
    return ask({ ...next, cursor: held.next_cursor }, readOrder, generation, session);
  }

  function more() {
    if (!available() || !ticket || refused) return Promise.resolve(false);
    return pending || track(continuePage);
  }

  return { read, refresh, more, dispose() { disposed = true; serial += 1; } };
}

const canContinue = (held, ticket) => held && ticket && held.generation === ticket.generation
  && !held.refreshing && !held.stale && !held.complete && typeof held.next_cursor === "string";

const stopsContinuation = (error) => ["task_query_basis_changed", "task_query_marker_unsupported",
  "task_query_timestamp_unsupported"].includes(error?.details?.reason);
