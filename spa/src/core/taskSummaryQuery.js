// Canonical membership identity for the negotiated tasks.list summary shape.
// Page size and cursor describe a read, never the query's membership.
const asciiLower = (value) => value.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
const orderedSet = (values) => [...new Map(values.map((value) => [JSON.stringify(value), value])).entries()]
  .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0).map(([, value]) => value);
const statusSlug = (value) => {
  const key = asciiLower(value.trim());
  return ({ "in progress": "in_progress", "in review": "in_review" })[key] || key;
};

function validateActorKeys(value) {
  const keys = value?.kind === "agent" ? ["kind", "agent_id"] : ["kind"];
  if (!value || typeof value !== "object" || Object.keys(value).some((key) => !keys.includes(key))) throw new Error("Invalid assignee filter");
}

function actorKey(value) {
  if (value === "any" || value === "none") return value;
  validateActorKeys(value);
  if (value.kind === "agent" && typeof value.agent_id === "string") return { kind: "agent", agent_id: value.agent_id };
  if (value.kind === "user" || value.kind === "project_agent") return { kind: value.kind };
  throw new Error("Invalid assignee filter");
}

const dimensions = [
  ["state", "states", (value) => value],
  ["status", "statuses", statusSlug],
  ["label", "labels", asciiLower],
  ["assignee", "assignees", actorKey],
];

function selected(params, scalar, plural, normalize) {
  if (Object.hasOwn(params, scalar) && Object.hasOwn(params, plural)) throw new Error(`Conflicting ${scalar} filters`);
  const values = Object.hasOwn(params, plural) ? params[plural] : params[scalar] == null ? [] : [params[scalar]];
  if (!Array.isArray(values)) throw new Error(`Invalid ${plural} selection`);
  return orderedSet(values.map(normalize));
}

function validSelector(value, valid) {
  if (!valid) throw new Error("Invalid summary selector");
  return value;
}

const booleanSelector = (value) => validSelector(value, typeof value === "boolean");
const enumSelector = (...choices) => (value) => validSelector(value, choices.includes(value));
const opaqueId = (value) => validSelector(value, typeof value === "string" && value.length > 0
  && new TextEncoder().encode(value).length <= 128);

function excludedActor(value) {
  validateActorKeys(value);
  if (value.kind === "agent") opaqueId(value.agent_id);
  return actorKey(value);
}

const exactSelectors = {
  watched: ["listWatched", booleanSelector],
  attention_rule: ["listAttention", enumSelector("asked", "legacy")],
  linked_workspace_id: ["listLinkedWorkspace", opaqueId],
  tracker_agent_id: ["listTracked", opaqueId],
  exclude_review_mode: ["listReviewMode", enumSelector("pull_request", "snapshot")],
  order: ["listOrder", enumSelector("number", "updated", "priority", "anchor")],
  done_since: ["listOrder", (value) => validSelector(value, Number.isSafeInteger(value))],
  unfinished: ["listReadiness", booleanSelector],
  blocked: ["listReadiness", booleanSelector],
  held: ["listReadiness", booleanSelector],
  undelivered: ["listReadiness", booleanSelector],
  exclude_assignee: ["listExcludeAssignee", excludedActor],
};

/** Unrestricted UI selections are omitted. Actor IDs keep their exact case;
 * only the backend's ASCII label folding and status aliases are shared. */
export function canonicalSummaryFilters(params = {}) {
  const allowed = [...dimensions.flatMap(([scalar, plural]) => [scalar, plural]), ...Object.keys(exactSelectors)];
  if (Object.keys(params).some((key) => !allowed.includes(key))) throw new Error("Unsupported summary selector");
  const result = {};
  for (const [scalar, plural, normalize] of dimensions) {
    const values = selected(params, scalar, plural, normalize);
    if (values.length) result[plural] = values;
  }
  for (const [name, [, normalize]] of Object.entries(exactSelectors)) {
    if (Object.hasOwn(params, name)) result[name] = normalize(params[name]);
  }
  // The bridge canonicalizes omitted order to Number before binding cursors.
  if (result.order === "number") delete result.order;
  return result;
}

/** Check original filters so an explicit default order still requires its
 * advertised feature. Canonical membership identity may omit that default. */
export function summaryFiltersSupported(filters = {}, taskCaps) {
  canonicalSummaryFilters(filters);
  return taskCaps?.listSummaries === true && Object.entries(exactSelectors)
    .every(([name, [capability]]) => !Object.hasOwn(filters, name) || taskCaps[capability] === true);
}
