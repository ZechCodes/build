// Test-only static attribution avoids allocating a stack for every storage read.
export function instrumentReadCallsites(source, id) {
  if (!id.includes("/spa/src/") || id.endsWith("/localCache.js")) return source;
  const file = id.split("/spa/src/")[1];
  let changed = false;
  const lines = source.split("\n").map((line, index) => line.replace(/\b(readCachedMany|readCached|captureCachedRecord)\(/g, (_, name) => {
    changed = true;
    return `__profileCacheRead(${JSON.stringify(`${file}:${index + 1}`)}, ${name}, `;
  }).replace(/\.map\(captureCachedRecord\)/g, () => {
    changed = true;
    return `.map((address) => __profileCacheRead(${JSON.stringify(`${file}:${index + 1}`)}, captureCachedRecord, address))`;
  }));
  return changed ? lines.join("\n") + '\nfunction __profileCacheRead(site, run, address) { window.__liveSyncMetrics?.attribution?.read(site, address); return run(address); }\n' : source;
}

export function instrumentStorageReadCallsites(source, id) {
  if (!id.endsWith("/src/core/localCache.js")) return source;
  return source.split("\n").map((line, index) => line.replace(/store\.get\(/g, `__profileStoreGet("core/localCache.js:${index + 1}", store, `)).join("\n")
    + '\nfunction __profileStoreGet(site, store, key) { window.__liveSyncMetrics?.storageAttribution?.read(site, {kind: String(key).split("|")[2] || "other"}); return store.get(key); }\n';
}

export function createReadAttribution(clock, phase, intervalMs = 10000) {
  const started = clock(), intervals = [];
  return { intervals, read(site, input) {
    const interval = Math.floor((clock() - started) / intervalMs);
    let bucket = intervals.find((held) => held.interval === interval);
    if (!bucket) { bucket = { interval, reads: 0, byKind: {}, byCallsite: {}, byCallsiteKind: {}, byPhase: {} }; intervals.push(bucket); }
    for (const address of Array.isArray(input) ? input : [input]) {
      bucket.reads++;
      const kind = address?.kind || "other", current = phase();
      bucket.byKind[kind] = (bucket.byKind[kind] || 0) + 1;
      bucket.byCallsite[site] = (bucket.byCallsite[site] || 0) + 1;
      bucket.byCallsiteKind[`${site}|${kind}`] = (bucket.byCallsiteKind[`${site}|${kind}`] || 0) + 1;
      bucket.byPhase[current] = (bucket.byPhase[current] || 0) + 1;
    }
  } };
}
