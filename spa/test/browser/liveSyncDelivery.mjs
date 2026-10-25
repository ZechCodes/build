// Pure reporting: does not alter the browser capture or infer unmeasured stalls.
const percentile = (values, fraction = .95) => [...values].sort((a, b) => a - b)[Math.max(0, Math.ceil(values.length * fraction) - 1)] ?? null;
const distribution = (values) => ({ samples: values.length, p95Ms: percentile(values), maxMs: values.length ? Math.max(...values) : null });

export function unionIntervals(intervals) {
  const union = [];
  for (const { start, end } of [...intervals].sort((a, b) => a.start - b.start)) {
    if (end < start) throw new Error("Invalid measured hold interval");
    if (union.length && start <= union.at(-1).end) union.at(-1).end = Math.max(union.at(-1).end, end);
    else union.push({ start, end });
  }
  return union;
}

export function summarizeLiveSyncDelivery(result) {
  const stages = result.ui.stages.filter((stage) => stage.entityId === "run-0");
  const receipts = new Map();
  for (const event of stages.filter((event) => event.stage === "receipt")) {
    for (const sequence of event.sequences.filter((sequence) => sequence > 1100)) {
      receipts.set(sequence, Math.min(receipts.get(sequence) ?? Infinity, event.at));
    }
  }
  const holdsMeasured = Array.isArray(result.ownRepairHolds);
  const holds = unionIntervals(result.ownRepairHolds || []);
  const raw = [], adjusted = [], late = [], commits = [];
  const completeness = { receipt: receipts.size, admission: 0, workerStart: 0, cacheCommit: 0, mountedPaint: 0 };
  const unorderedStageIntervals = {};
  const stageLatencies = { receiptToAdmission: [], admissionToWorker: [], workerToCommit: [], commitToPaint: [] };
  for (const [sequence, receipt] of receipts) {
    const at = { receipt };
    for (const kind of ["admission", "workerStart", "cacheCommit", "mountedPaint"]) {
      const events = stages.filter((event) => event.stage === kind && event.at >= receipt
        && event.sequences.some((covered) => kind === "mountedPaint" ? covered === sequence : covered >= sequence));
      if (events.length) { at[kind] = Math.min(...events.map((event) => event.at)); completeness[kind]++; }
    }
    for (const [key, first, last] of [["receiptToAdmission", "receipt", "admission"], ["admissionToWorker", "admission", "workerStart"], ["workerToCommit", "workerStart", "cacheCommit"], ["commitToPaint", "cacheCommit", "mountedPaint"]]) {
      if (at[first] === undefined || at[last] === undefined) continue;
      if (at[last] >= at[first]) stageLatencies[key].push(at[last] - at[first]);
      else unorderedStageIntervals[key] = (unorderedStageIntervals[key] || 0) + 1;
    }
    if (at.cacheCommit !== undefined) commits.push(at.cacheCommit - receipt);
    if (at.mountedPaint === undefined) continue;
    const latency = at.mountedPaint - receipt;
    raw.push(latency);
    const overlap = holds.reduce((total, hold) => total + Math.max(0, Math.min(at.mountedPaint, hold.end) - Math.max(receipt, hold.start)), 0);
    if (holdsMeasured) adjusted.push(latency - overlap);
    if (holdsMeasured && (!holds.length || receipt >= holds.at(-1).end)) late.push(latency);
  }
  const expected = result.framesReceived === undefined ? receipts.size : Math.ceil(result.framesReceived / 6);
  return { quantile: "nearest rank ceil(n*q)-1", expectedTips: expected, completeness,
    complete: Object.values(completeness).every((count) => count === expected),
    holdAdjustmentAvailable: holdsMeasured, measuredOwnRepairHoldUnion: holds,
    receiptToCommit: distribution(commits),
    raw: distribution(raw), holdAdjusted: holdsMeasured ? distribution(adjusted) : null,
    lateUnheld: holdsMeasured ? distribution(late) : null,
    stageIntervalsAreCoverageObservations: true, unorderedStageIntervals,
    stageLatencies: Object.fromEntries(Object.entries(stageLatencies).map(([key, values]) => [key, distribution(values)])) };
}
