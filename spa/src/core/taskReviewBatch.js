// A revision owns its local drafts; the durable write outbox owns submission.
import { uiAddress } from "./localUiState.js";
import { readUiRecord, subscribeUiRecords, updateUiRecord } from "./localUiStore.js";
import { provenExpiredFailure } from "./writeOperationIntent.js";

const empty = () => ({ drafts: [], summary: "", submission: null });
const nonempty = (draft) => Boolean(draft.body?.trim());
const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const successful = (row) => row?.acknowledged && !row.unresolved && row.state === "succeeded";
const failure = (row) => row?.state === "failed" || row?.unresolved || row?.state === "result_expired";
const failureMessage = (row) => row?.error?.message || row?.confirmation?.error?.message || "A pending comment could not be sent. Edit the pending comments and try again.";
const paramsFor = (taskId, draft) => ({ task_id: taskId, body: draft.body.trim(),
  ...(draft.anchor ? { anchor: draft.anchor } : {}), ...(draft.replyTo ? { reply_to: draft.replyTo } : {}) });

export function reviewBatchAnchorLink(taskNumber, snapshotNumber, anchor) {
  const mode = anchor.commit ? `commit/${anchor.commit}` : "all";
  const side = anchor.side === "old" ? "old-" : "";
  return `[[#${taskNumber}:r${snapshotNumber}:${mode}:${anchor.directory_id}:${anchor.path}#${side}L${anchor.line}]]`;
}

export function reviewBatchVerdictBody(summary, drafts, taskNumber, snapshotNumber, priorAnchors = []) {
  const anchors = uniqueBatchAnchors([...priorAnchors, ...drafts.flatMap((draft) => draft.anchor ? [draft.anchor] : [])]);
  const links = anchors.map((anchor) => `- ${reviewBatchAnchorLink(taskNumber, snapshotNumber, anchor)}`);
  return [summary.trim(), links.join("\n")].filter(Boolean).join("\n\n");
}

function uniqueBatchAnchors(anchors) {
  const identity = (anchor) => JSON.stringify([anchor.snapshot_id, anchor.directory_id, anchor.path, anchor.side, anchor.line, anchor.commit || ""]);
  return [...new Map(anchors.map((anchor) => [identity(anchor), anchor])).values()];
}

function submissionOperations(options, drafts, summary, verdict, uuid, priorAnchors) {
  const { taskId, taskNumber, snapshot } = options;
  const params = drafts.map((draft) => paramsFor(taskId, draft));
  if (verdict) params.push({ task_id: taskId,
    body: reviewBatchVerdictBody(summary, drafts, taskNumber, snapshot.number, priorAnchors) || (verdict === "approve" ? "Approved." : "Changes requested."),
    opinion: { snapshot_id: snapshot.id, verdict } });
  else if (summary.trim()) params.push({ task_id: taskId, body: summary.trim() });
  let previous = null;
  return params.map((params, index) => {
    const operationId = uuid();
    const operation = { operationId, method: "tasks.comment", params, after: previous ? [previous] : [],
      owner: { kind: "review-batch", taskId, projectId: options.projectId, snapshotId: snapshot.id },
      draft: drafts[index] || { body: params.body } };
    previous = operationId;
    return operation;
  });
}

function recoverablySettled(row) {
  return successful(row) || (row.state === "failed" && row.effect === "unapplied") || provenExpiredFailure(row);
}

function recoverableFailure(bad, rows) {
  if (!bad || bad.unresolved || bad.effect !== "unapplied") return false;
  return !rows.some((row) => row?.unresolved || (row?.transportAttempted && !recoverablySettled(row)));
}

function legacyBatchValue(legacy, uuid) {
  if (typeof legacy?.body !== "string" || !legacy.body.trim()) return null;
  if (!legacy.anchor && !legacy.replyTo) return { ...empty(), summary: legacy.body };
  return { ...empty(), drafts: [{ id: uuid(), body: legacy.body, anchor: legacy.anchor || null,
    replyTo: legacy.replyTo || null, view: "changes" }] };
}

function removeSent(state, submission, acknowledgedIds) {
  const sent = submission.drafts.filter((draft, index) => acknowledgedIds.has(submission.operations[index]?.operationId));
  return { ...state, drafts: state.drafts.filter((draft) => !sent.some((held) => held.id === draft.id && same(held, draft))),
    summary: state.summary === submission.summary ? "" : state.summary, submission: null };
}

/** All changes commit transactionally, then paint from the durable UI record.
 * enqueueBatch resolves at persistence; the submission stays visible until
 * every comment has succeeded. Existing operation IDs survive remounts. */
export function createTaskReviewBatch(options) {
  const { deviceId, projectId, taskId, snapshot, writeOperations, callRpc, onChange = () => {} } = options;
  const uuid = options.randomUUID || (() => crypto.randomUUID());
  const address = uiAddress({ deviceId, entityId: projectId, kind: "draft", view: "task-review-batch", sub: JSON.stringify([taskId, snapshot.id]) });
  let value = empty();
  let disposed = false;
  let edits = Promise.resolve();
  let readSerial = 0;
  let submitting = null;

  const operationRows = () => writeOperations?.readOperations() || [];
  function read() {
    const submission = value.submission;
    if (!submission) return { ...value, error: "" };
    const rows = operationRows();
    const bad = submission.operations.map((operation) => rows.find((row) => row.operationId === operation.operationId)).find(failure);
    const error = bad ? failureMessage(bad) : submission.enqueueError || "";
    return { ...value, submission: { ...submission, status: error ? "failed" : "queued" }, error };
  }
  async function hydrate() {
    const serial = ++readSerial;
    const held = await readUiRecord(address);
    if (disposed || serial !== readSerial) return;
    value = { ...empty(), ...held?.value };
    onChange(read());
  }
  async function adoptLegacyDraft() {
    const oldAddress = uiAddress({ deviceId, entityId: projectId, kind: "draft", view: "task-review-feedback", sub: `${taskId}:${snapshot.id}` });
    const [current, legacy] = await Promise.all([readUiRecord(address), readUiRecord(oldAddress)]);
    if (current) return;
    const migrated = legacyBatchValue(legacy?.value, uuid);
    if (migrated) await updateUiRecord(address, (held) => held === undefined ? migrated : held);
  }
  const ready = adoptLegacyDraft().then(hydrate).then(async () => {
    if (!disposed && value.submission && writeOperations?.enqueueBatch) {
      try { await enqueuePersisted(value.submission); }
      catch { /* The retained enqueue error makes this submission retryable. */ }
    }
  });
  function change(update) {
    const task = edits.then(async () => {
      await ready;
      await updateUiRecord(address, (held) => update({ ...empty(), ...held }));
      await hydrate();
    });
    edits = task.catch(() => {});
    return task;
  }
  const unwatch = subscribeUiRecords(address, () => { void hydrate(); });

  async function reconcile() {
    if (disposed || !value.submission || !writeOperations) return;
    const submission = value.submission;
    const rows = operationRows();
    const acknowledged = submission.operations.filter((operation) => successful(rows.find((row) => row.operationId === operation.operationId)));
    if (acknowledged.length !== submission.operations.length) { onChange(read()); return; }
    const ids = new Set(acknowledged.map((operation) => operation.operationId));
    await change((state) => state.submission?.id === submission.id ? removeSent(state, submission, ids) : state);
  }
  const stopOperations = writeOperations?.subscribe(() => { void reconcile(); }) || (() => {});

  async function prepare(verdict) {
    await change((state) => {
      if (state.submission) return state;
      const drafts = state.drafts.filter(nonempty);
      const priorAnchors = verdict ? (options.priorAnchors?.() || []).filter((anchor) => anchor.snapshot_id === snapshot.id) : [];
      const operations = submissionOperations(options, drafts, state.summary, verdict, uuid, priorAnchors);
      if (!operations.length) return state;
      return { ...state, submission: { id: uuid(), drafts, priorAnchors, summary: state.summary, verdict, operations, acknowledged: [] } };
    });
    return value.submission;
  }
  async function sendFallback(submission) {
    const acknowledged = new Set(submission.acknowledged);
    for (const operation of submission.operations) {
      if (acknowledged.has(operation.operationId)) continue;
      await callRpc(operation.method, operation.params, { operationId: operation.operationId, after: operation.after, owner: operation.owner, draft: operation.draft });
      acknowledged.add(operation.operationId);
      await change((state) => state.submission?.id === submission.id ? { ...state, submission: { ...state.submission, acknowledged: [...acknowledged] } } : state);
    }
    await change((state) => state.submission?.id === submission.id ? removeSent(state, submission, acknowledged) : state);
    return [...acknowledged];
  }
  async function send(verdict) {
    const submission = await prepare(verdict);
    if (!submission) return [];
    if (!writeOperations?.enqueueBatch) return sendFallback(submission);
    const ids = await enqueuePersisted(submission);
    await reconcile();
    return ids;
  }
  async function enqueuePersisted(submission) {
    try {
      const ids = await writeOperations.enqueueBatch(submission.operations);
      await saveEnqueueError(submission.id, null);
      return ids;
    } catch (error) {
      await saveEnqueueError(submission.id, String(error?.message || error));
      throw error;
    }
  }
  async function saveEnqueueError(id, enqueueError) {
    await updateUiRecord(address, (held) => held?.submission?.id === id
      ? { ...held, submission: { ...held.submission, enqueueError } } : held);
    await hydrate();
  }
  async function retry() {
    await ready;
    await edits;
    const submission = value.submission;
    if (!submission) return [];
    if (!writeOperations?.enqueueBatch) return sendFallback(submission);
    const ids = await enqueuePersisted(submission);
    await reconcile();
    return ids;
  }
  function startSubmission(action) {
    if (submitting) return submitting;
    submitting = action().finally(() => { submitting = null; });
    return submitting;
  }
  async function recover() {
    await edits;
    const submission = value.submission;
    if (!submission) return;
    const rows = operationRows();
    const held = submission.operations.map((operation) => rows.find((row) => row.operationId === operation.operationId));
    const bad = held.find(failure);
    if (!recoverableFailure(bad, held)) throw new Error("The result is still being checked. These comments remain pending.");
    const unsent = held.filter((row) => row && !row.acknowledged && !row.transportAttempted && row.state === "queued");
    await writeOperations.cancelUnsentBatch(unsent.map((row) => row.operationId));
    const sentIds = new Set(held.filter(successful).map((row) => row.operationId));
    await change((state) => {
      if (state.submission?.id !== submission.id) return state;
      const restored = removeSent(state, submission, sentIds);
      return { ...restored, summary: state.summary };
    });
  }

  void ready.then(reconcile).catch(() => {});
  return {
    ready, read,
    async add(draft) {
      const id = uuid();
      await change((state) => ({ ...state, drafts: [...state.drafts, { id, body: "", ...draft }] }));
      return id;
    },
    edit: (id, changes) => change((state) => ({ ...state, drafts: state.drafts.map((draft) => draft.id === id ? { ...draft, ...changes, id } : draft) })),
    remove: (id) => change((state) => ({ ...state, drafts: state.drafts.filter((draft) => draft.id !== id) })),
    summary: (summary) => change((state) => ({ ...state, summary })),
    flush: () => edits,
    submit(verdict = "") {
      if (!["", "approve", "request_changes"].includes(verdict)) return Promise.reject(new Error("Invalid review verdict"));
      return startSubmission(() => send(verdict));
    },
    retry: () => startSubmission(retry),
    recover,
    dispose() { disposed = true; unwatch(); stopOperations(); },
  };
}
