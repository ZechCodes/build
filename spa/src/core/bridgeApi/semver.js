// Semantic versions, as much of them as the wire contract needs and no more.
//
// The bridge reports `api_version` in its greeting; this SPA carries one
// adapter per API major and picks by that string. That is the whole job: parse
// a version, order two of them, and answer whether one falls inside a range
// like ">=1.0.0 <2.0.0". No dependency — a version compare is not worth a
// package, and the rules here (a prerelease sorts below its release, build
// metadata is not compared) are the only ones the contract leans on.

const VERSION_PATTERN = /^(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

/** `{major, minor, patch, prerelease}`, or null for anything that is not a
 *  version. Missing parts read as zero, so `1` is `1.0.0`. */
export function parse(version) {
  if (typeof version !== "string") return null;
  const match = VERSION_PATTERN.exec(version.trim());
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2] || 0),
    patch: Number(match[3] || 0),
    prerelease: match[4] || "",
  };
}

const sign = (delta) => (delta > 0 ? 1 : delta < 0 ? -1 : 0);

/** Prereleases only ever meet their own release here, so the rule is the one
 *  line of it that matters: `1.1.0-rc.1` is below `1.1.0`. */
function comparePrerelease(left, right) {
  if (left === right) return 0;
  if (!left) return 1;
  if (!right) return -1;
  return left < right ? -1 : 1;
}

/** -1, 0 or 1. Two versions that cannot be parsed compare equal; one that
 *  cannot sorts below one that can, so junk never wins a range. */
export function compare(a, b) {
  const left = parse(a);
  const right = parse(b);
  if (!left || !right) return sign((left ? 1 : 0) - (right ? 1 : 0));
  const core =
    sign(left.major - right.major) || sign(left.minor - right.minor) || sign(left.patch - right.patch);
  return core || comparePrerelease(left.prerelease, right.prerelease);
}

const COMPARATORS = {
  ">=": (order) => order >= 0,
  "<=": (order) => order <= 0,
  ">": (order) => order > 0,
  "<": (order) => order < 0,
  "=": (order) => order === 0,
};

const OPERATORS = [">=", "<=", ">", "<", "="];

function splitComparator(text) {
  const operator = OPERATORS.find((candidate) => text.startsWith(candidate));
  return operator ? { operator, bound: text.slice(operator.length) } : { operator: "=", bound: text };
}

function meets(version, text) {
  const { operator, bound } = splitComparator(text);
  if (!parse(bound)) return false;
  return COMPARATORS[operator](compare(version, bound));
}

/** Whether `version` falls inside a space-separated comparator list, e.g.
 *  ">=1.0.0 <2.0.0". `*` admits every version. An unparseable version, an
 *  empty range or a comparator naming an unparseable bound is false, never a
 *  throw: a bridge that reports nonsense is a bridge no adapter claims. */
export function satisfies(version, range) {
  if (!parse(version) || typeof range !== "string") return false;
  const terms = range.trim().split(/\s+/).filter(Boolean);
  if (!terms.length) return false;
  if (terms.length === 1 && terms[0] === "*") return true;
  return terms.every((term) => meets(version, term));
}
