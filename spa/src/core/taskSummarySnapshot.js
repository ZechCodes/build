// Evidence for one raw server query chain. It never certifies the independently
// selected normalized rows, watch families, absence, totals or write receipts.
import { canonicalSummaryFilters, summarySnapshotFiltersIndependent } from "./taskSummaryQuery.js";

const lifetimeFields = ["storeGeneration", "scopeGeneration", "retirementEpoch"];
const positive = (value) => Number.isSafeInteger(value) && value > 0;
const nonempty = (value) => typeof value === "string" && value.length > 0;
const queryKey = (filters) => JSON.stringify(canonicalSummaryFilters(filters));
const sameLifetime = (proof, context) => lifetimeFields.every((field) =>
  nonempty(proof?.[field]) && proof[field] === context?.[field]);
const sameScope = (left, right) => left?.deviceId === right?.deviceId && left?.projectId === right?.projectId;
const sameIds = (before, after) => Array.isArray(before) && Array.isArray(after)
  && before.length === after.length && before.every((id, index) => id === after[index]);

function capturedPage(contentRead, read, context) {
  return positive(read) && contentRead?.readOrder === read && sameLifetime(contentRead.ticket, context);
}

/** A known captured ordinary lifetime was replaced while this request was
 * outstanding. Missing legacy evidence or unsupported query classes are not
 * retirement signals; they retain their existing compatibility behavior. */
export function summaryPageReadRetired(contentRead, read, context) {
  return positive(read) && contentRead?.readOrder === read
    && lifetimeFields.every((field) => nonempty(contentRead.ticket?.[field]))
    && !sameLifetime(contentRead.ticket, context);
}

function addressedQuery(ticket, key) {
  const address = ticket.address, scope = ticket.scope;
  return address?.deviceId === scope.deviceId && address.entityId === String(scope.projectId)
    && address.kind === "tracker-summary-query" && address.sub === key;
}

function sameChain(query, proof) {
  return positive(proof.generation) && proof.generation === query.generation
    && proof.generation === query.published_generation && positive(proof.read_order)
    && proof.read_order === query.observed_read_order;
}

function samePosition(query, proof) {
  return nonempty(proof.basis) && proof.basis === query.basis && sameIds(proof.ids, query.ids)
    && proof.next_cursor === query.next_cursor && proof.complete === query.complete;
}

function applicableSnapshot(query, scope, filters, context) {
  const proof = query?.snapshot_membership;
  if (!proof || proof.version !== 1) return false;
  return sameScope(proof.scope, scope) && proof.query_key === queryKey(filters)
    && sameChain(query, proof) && samePosition(query, proof) && sameLifetime(proof, context);
}

/** Called inside the page publication transaction after existing cursor,
 * acceptance and relation-provenance validation. A partial page cannot repair
 * missing legacy evidence or cross an ordinary lifetime/retirement boundary. */
export function snapshotPageEvidence(previous, ticket, page, read, context) {
  if (!summarySnapshotFiltersIndependent(ticket.filters) || !nonempty(page.basis)
    || !capturedPage(ticket.contentRead, read, context)) return null;
  const key = queryKey(ticket.filters);
  if (!addressedQuery(ticket, key)) return null;
  const first = ticket.cursor === null;
  if (!first && !applicableSnapshot(previous, ticket.scope, ticket.filters, context)) return null;
  const priorIds = first ? [] : previous.ids;
  return { version: 1, scope: { deviceId: ticket.scope.deviceId, projectId: ticket.scope.projectId },
    query_key: key, basis: page.basis, generation: ticket.generation, read_order: read,
    ...Object.fromEntries(lifetimeFields.map((field) => [field, context[field]])),
    ids: [...new Set([...priorIds, ...page.tasks.map((task) => task.id)])],
    next_cursor: page.next_cursor ?? null, complete: page.next_cursor == null };
}

/** This result permits only an explicit next-page request. Composite stale,
 * mixed normalized data and missing display entities remain separate facts.
 * The reader still checks its live capability/session/mutation/ticket guards. */
export function snapshotContinuationEligible(query, { scope, filters, context, overtaken }) {
  if (overtaken || !summarySnapshotFiltersIndependent(filters)) return false;
  return !query.refreshing && !query.complete && nonempty(query.next_cursor)
    && applicableSnapshot(query, scope, filters, context);
}
