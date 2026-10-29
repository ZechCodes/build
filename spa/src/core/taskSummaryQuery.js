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
  ["state", "states", (value) => value.trim()],
  ["status", "statuses", statusSlug],
  ["label", "labels", (value) => asciiLower(value.trim())],
  ["assignee", "assignees", actorKey],
];

function selected(params, scalar, plural, normalize) {
  if (Object.hasOwn(params, scalar) && Object.hasOwn(params, plural)) throw new Error(`Conflicting ${scalar} filters`);
  const values = Object.hasOwn(params, plural) ? params[plural] : params[scalar] == null ? [] : [params[scalar]];
  if (!Array.isArray(values)) throw new Error(`Invalid ${plural} selection`);
  return orderedSet(values.map(normalize));
}

/** Unrestricted UI selections are omitted. Actor IDs keep their exact case;
 * only the backend's ASCII label folding and status aliases are shared. */
export function canonicalSummaryFilters(params = {}) {
  const allowed = dimensions.flatMap(([scalar, plural]) => [scalar, plural]);
  if (Object.keys(params).some((key) => !allowed.includes(key))) throw new Error("Unsupported summary selector");
  const result = {};
  for (const [scalar, plural, normalize] of dimensions) {
    const values = selected(params, scalar, plural, normalize);
    if (values.length) result[plural] = values;
  }
  return result;
}
