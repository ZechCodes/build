// Instrumentation only: this browser module is never imported by product code.
export function installLiveSyncMetrics() {
  const metrics = { active: false, counters: {}, timings: {}, reads: {}, readStacks: {}, stages: [], drawCallsites: {}, stack: [] };
  metrics.count = (name, amount = 1) => {
    if (metrics.active) metrics.counters[name] = (metrics.counters[name] || 0) + amount;
  };
  metrics.time = (name, run) => {
    if (!metrics.active) return run();
    if (name === "inboxDraw") {
      const callsite = new Error().stack.split("\n").slice(3, 6).join("\n");
      metrics.drawCallsites[callsite] = (metrics.drawCallsites[callsite] || 0) + 1;
    }
    const frame = { at: performance.now(), children: 0 };
    metrics.stack.push(frame);
    try { return run(); } finally {
      const elapsed = performance.now() - frame.at;
      metrics.stack.pop();
      const held = metrics.timings[name] ||= { count: 0, inclusiveMs: 0, selfMs: 0, maxMs: 0 };
      held.count++; held.inclusiveMs += elapsed; held.selfMs += elapsed - frame.children;
      held.maxMs = Math.max(held.maxMs, elapsed);
      if (metrics.stack.length) metrics.stack.at(-1).children += elapsed;
    }
  };
  metrics.read = (address) => {
    if (!metrics.active) return;
    const key = `${address.kind}|${address.entityId || ""}|${address.sub || ""}`;
    metrics.reads[key] = (metrics.reads[key] || 0) + 1;
    if (!metrics.readStacks[key]) metrics.readStacks[key] = new Error().stack.split("\n").slice(2, 8);
  };
  metrics.stage = (stage, item) => {
    if (!metrics.active) return;
    metrics.stages.push({ stage, at: performance.now(), entityId: item?.entity_id,
      sequences: (item?.thread || []).flatMap((tip) => [...new Set([tip.last_sequence, ...(tip.items || []).map((row) => row.data?.sequence)])]) });
  };
  const canvasRects = new WeakMap();
  const originalRect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function (...args) {
    const canvas = this instanceof HTMLCanvasElement;
    return metrics.time(canvas ? "canvasGeometry" : "otherGeometry", () => {
      if (canvas && window.__liveSyncCacheCanvasRect && canvasRects.has(this)) { metrics.count("canvasGeometryCacheHits"); return canvasRects.get(this); }
      if (canvas) metrics.count("canvasNativeGeometry");
      const rect = originalRect.apply(this, args);
      if (canvas && window.__liveSyncCacheCanvasRect) canvasRects.set(this, rect);
      return rect;
    });
  };
  const observer = new MutationObserver((records) => {
    metrics.count("domMutationRecords", records.length);
    metrics.count("inboxMutationRecords", records.filter((record) => record.target.closest?.("#inbox-list")).length);
  });
  observer.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
  window.__liveSyncMetrics = metrics;
  return metrics;
}
