const HOUR_SECONDS = 60 * 60;
const DAY_SECONDS = 24 * HOUR_SECONDS;
const plural = (amount, unit) => `${amount} ${unit}${amount === 1 ? "" : "s"}`;

/** The lifetime stamped on a metadata-only secret chip. */
export function secretTtlLabel(ttlSeconds) {
  if (ttlSeconds === null) return "No expiry";
  const seconds = Number(ttlSeconds);
  if (!Number.isSafeInteger(seconds) || seconds <= 0) return "";
  if (seconds % DAY_SECONDS === 0) return `${plural(seconds / DAY_SECONDS, "day")} after sending`;
  if (seconds % HOUR_SECONDS === 0) return `${plural(seconds / HOUR_SECONDS, "hour")} after sending`;
  if (seconds % 60 === 0) return `${plural(seconds / 60, "minute")} after sending`;
  return `${plural(seconds, "second")} after sending`;
}

/** Exact visible chip copy from a validated cache descriptor. */
export function secretChipLabel(metadata) {
  const name = metadata?.name;
  const scope = { agent: "Agent", project: "Project", environment: "Environment" }[metadata?.grant?.scope];
  const ttl = secretTtlLabel(metadata?.ttl_seconds);
  if (![name, scope, ttl].every(Boolean)) return "";
  return `${name} · ${scope} · ${ttl}`;
}


/** Only live grants can shadow another live name. Staged uploads acquire
 * authority when the user's own message is accepted. */
export function secretOverridesEnvironment(secret, entries = []) {
  return secret?.grant?.scope === "project" && secret.status === "available"
    && entries.some(entry => entry.grant?.scope === "environment" && entry.status === "available" && entry.name === secret.name);
}
