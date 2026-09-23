import { createThreadCache, createThreadState } from "./thread.js";
import { createOptimisticStore } from "./optimistic.js";
import { createChatChoiceController } from "./chatChoiceController.js";
import { normalizeViewingContext } from "./viewingContext.js";
import { uiAddress, watchUiState } from "./localUiState.js";

const EMPTY_DRAFT = Object.freeze({ body: "", attachments: [] });
export const CHAT_LOCAL_STATE_PREFIX = "build.chat.v1:";
const OPERATION_STATES = new Set(["queued", "claimed", "delivered", "uncertain"]);
const REQUIRED_OPERATION_RECEIPT_FIELDS = [
  "operation_id",
  "entity_id",
  "agent_id",
  "conversation_id",
  "posted_sequence",
  "operation_status",
];

/** The message with nothing said about where it was written — what goes to a
 *  bridge that does not take context, and what a stamp alone leaves behind when
 *  it turns out to name nothing. */
const withoutViewingContext = (message) => {
  if (!message.viewing_context) return message;
  const rest = { ...message };
  delete rest.viewing_context;
  return rest;
};

const cloneAttachments = (attachments = []) => attachments.map((attachment) => ({ ...attachment }));
// An upload in flight carries a Promise and often a megabyte thumbnail. Those
// are live resources, not a durable draft; only its descriptor can survive a
// reload. A pending upload returns as a retryable chip without cloning bytes.
const storedAttachments = (attachments = []) => attachments.map((attachment) => ({
  name: attachment.name,
  size: attachment.size,
  mime: attachment.mime,
  status: attachment.status === "uploading" ? "failed" : attachment.status,
  descriptor: attachment.descriptor || null,
  error: attachment.status === "uploading" ? "Attach this file again" : attachment.error || "",
}));
const deepFreeze = (value) => {
  if (!value || typeof value !== "object" || Object.isFrozen(value)) return value;
  Object.values(value).forEach(deepFreeze);
  return Object.freeze(value);
};

const scopeKeyOf = (scope) => {
  if (scope && typeof scope.key === "string") return scope.key;
  if (scope && typeof scope.id === "string") return scope.id;
  const accountId = (scope && scope.accountId) || "";
  const deviceId = (scope && scope.deviceId) || "";
  return `${accountId}:${deviceId}`;
};

const randomOperationId = () => {
  if (globalThis.crypto && typeof globalThis.crypto.randomUUID === "function") return globalThis.crypto.randomUUID();
  const random = Math.random().toString(36).slice(2);
  return `chat-${Date.now().toString(36)}-${random}`;
};

const controllerKey = ({ entityId = "", agentId = "", draftId = "" }) =>
  draftId ? `draft:${draftId}` : `agent:${entityId}:${agentId}`;

// A promoted controller keeps its draft id as lineage, but reloads address it
// by the durable entity/agent pair. Storage therefore prefers that pair once
// it exists, unlike the live controller map while promotion is in progress.
const storedControllerKey = ({ entityId = "", agentId = "", draftId = "" }) =>
  agentId ? `agent:${entityId}:${agentId}` : `draft:${draftId}`;

const storageOrNull = (provided) => {
  if (provided !== undefined) return provided;
  try {
    return globalThis.localStorage;
  } catch {
    return null;
  }
};

const recordOrEmpty = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : {};

const readLocalState = (storage, scopeKey) => {
  try {
    const value = JSON.parse(storage?.getItem(`${CHAT_LOCAL_STATE_PREFIX}${scopeKey}`) || "{}");
    return recordOrEmpty(value);
  } catch {
    return {};
  }
};

const writeLocalState = (storage, scopeKey, value) => {
  try {
    storage?.setItem(`${CHAT_LOCAL_STATE_PREFIX}${scopeKey}`, JSON.stringify(value));
  } catch {
    /* Storage can be unavailable; the repository remains usable for this tab. */
  }
};

const publicIdentity = (identity) => Object.freeze({
  entityId: identity.entityId || "",
  agentId: identity.agentId || "",
  conversationId: identity.conversationId || "",
  draftId: identity.draftId || "",
});

const draftSnapshot = (draft) => ({
  body: draft.body,
  attachments: cloneAttachments(draft.attachments),
  revision: draft.revision,
});

const messageSnapshot = ({ body = "", attachments = [], viewing_context, ...rest } = {}) => Object.freeze({
  ...rest,
  body,
  attachments: Object.freeze(cloneAttachments(attachments)),
  ...(viewing_context ? { viewing_context: deepFreeze(structuredClone(viewing_context)) } : {}),
});

const deliveryMayHaveStarted = (error) =>
  error?.uncertain === true || (error?.timedOut === true && error?.uncertain !== false);

/**
 * Whether the bridge just said it has never heard of this operation.
 *
 * That is the one refusal that settles an uncertain post the other way: the
 * ledger is durable and committed with the transcript mutation it acknowledges
 * (bridge/src/operation.rs), so an id it cannot find is an id that never
 * arrived, and re-sending it cannot duplicate anything. The code is what a 1.x
 * bridge names (`not_found`); the sentence is the 1.0 fallback, where a refusal
 * carries no code and the string is all there is.
 */
const neverReachedTheBridge = (error) =>
  error?.code === "not_found"
  || error?.error_code === "not_found"
  || /unknown operation_id/i.test(error?.message || "");

/** What one uncertain post's resolution amounts to, for the caller and the
 *  diagnostics — never a throw: one post that cannot be resolved must not stop
 *  the ones behind it in the queue. */
const resolution = (operationId, outcome, extra = {}) => ({ operationId, outcome, ...extra });

function assertAddress(identity) {
  if (!identity.entityId) throw new Error("Chat controller requires an entity id");
  if (!identity.agentId) throw new Error("Chat controller requires an agent id");
  if (!identity.conversationId) throw new Error("Chat controller requires a conversation id");
}

function validatePostReceipt(receipt, submission) {
  if (!receipt) throw new Error("thread.post returned no receipt");
  if (submission.threadPostOperations) {
    const missing = REQUIRED_OPERATION_RECEIPT_FIELDS.find((field) => receipt[field] === null || receipt[field] === undefined || receipt[field] === "");
    if (missing) throw new Error(`thread.post operation receipt is missing ${missing}`);
  }
  const mismatches = [
    ["operation_id", submission.operationId, "operation"],
    ["entity_id", submission.address.entityId, "entity"],
    ["agent_id", submission.address.agentId, "agent"],
    ["conversation_id", submission.address.conversationId, "conversation"],
  ];
  const mismatch = mismatches.find(([field, wanted]) => receipt[field] && receipt[field] !== wanted);
  if (mismatch) throw new Error(`thread.post returned a receipt for a different ${mismatch[2]}`);
  if (submission.threadPostOperations && !OPERATION_STATES.has(receipt.operation_status)) {
    throw new Error("thread.post returned an unknown operation status");
  }
  return receipt;
}

function validateStatusReceipt(receipt, submission) {
  if (!receipt) throw new Error("thread.operation returned no receipt");
  const required = ["operation_id", "status", "entity_id", "agent_id", "conversation_id", "choice_revision", "posted_sequence"];
  const missing = required.find((field) => receipt[field] === null || receipt[field] === undefined || receipt[field] === "");
  if (missing) throw new Error(`thread.operation receipt is missing ${missing}`);
  const comparable = {
    operation_id: submission.operationId,
    entity_id: submission.address.entityId,
    agent_id: submission.address.agentId,
    conversation_id: submission.address.conversationId,
  };
  const mismatch = Object.entries(comparable).find(([field, wanted]) => receipt[field] !== wanted);
  if (mismatch) throw new Error(`thread.operation returned a receipt for a different ${mismatch[0]}`);
  if (!OPERATION_STATES.has(receipt.status)) throw new Error("thread.operation returned an unknown operation status");
  return receipt;
}

class ChatController {
  #repository;
  #identity;
  #bound;
  #draft;
  #failures;
  #listeners;
  #choices;
  #operations;
  /** Every operation this controller has already re-sent unprompted, so the
   *  automatic recovery is once per post and not once per reconnect. */
  #resent;
  #threadState;
  #cacheDraft;
  #draftOwner;

  constructor(repository, identity) {
    this.#repository = repository;
    this.#draftOwner = randomOperationId();
    this.#identity = { ...identity };
    this.#bound = !identity.draftId;
    const saved = repository.readControllerState(identity);
    this.#draft = {
      body: !repository.cacheDrafts && typeof saved.draft?.body === "string" ? saved.draft.body : "",
      attachments: cloneAttachments(!repository.cacheDrafts && Array.isArray(saved.draft?.attachments) ? saved.draft.attachments : []),
      revision: 0,
      attachmentRevision: 0,
    };
    this.#failures = [];
    this.#listeners = new Set();
    this.#threadState = repository.history(identity.conversationId)?.threadState || createThreadState();
    this.#choices = createChatChoiceController({
      repository,
      identityOf: () => publicIdentity(this.#identity),
      announce: () => this.announce(),
      initial: saved.choice,
      onChange: (choice) => repository.writeControllerState(this.#identity, { choice }),
    });
    this.#operations = new Map();
    this.#resent = new Set();
    this.#startDraftCache();
  }

  #startDraftCache() {
    if (!this.#repository.cacheDrafts) return;
    this.#cacheDraft = watchUiState(this.#repository.draftAddress(this.#identity), (saved) => {
      if (!saved || typeof saved.body !== "string") return;
      const attachments = cloneAttachments(Array.isArray(saved.attachments) ? saved.attachments : []);
      if (saved.body !== this.#draft.body || JSON.stringify(attachments) !== JSON.stringify(this.#draft.attachments)) {
        this.#draft = {
          body: saved.body,
          attachments,
          revision: this.#draft.revision + 1,
          attachmentRevision: this.#draft.attachmentRevision + 1,
        };
      }
      this.announce();
    }, { debounceMs: 180 });
  }

  get identity() {
    return publicIdentity(this.#identity);
  }

  get historyIdentity() {
    return this.#repository.historyIdentity(this.#identity.conversationId);
  }

  get history() {
    return this.#repository.history(this.#identity.conversationId);
  }

  get threadState() {
    return this.#threadState;
  }

  readDraft() {
    return draftSnapshot(this.#draft);
  }

  writeDraft({ body = this.#draft.body, attachments = this.#draft.attachments } = {}) {
    this.#repository.assertActive();
    const replacesAttachments = arguments[0] && Object.hasOwn(arguments[0], "attachments");
    this.#draft = {
      body,
      attachments: cloneAttachments(attachments),
      revision: this.#draft.revision + 1,
      attachmentRevision: this.#draft.attachmentRevision + (replacesAttachments ? 1 : 0),
    };
    this.#repository.writeControllerState(this.#identity, { draft: this.#draft });
    this.#cacheDraft?.schedule({ body: this.#draft.body, attachments: storedAttachments(this.#draft.attachments), owner: this.#draftOwner });
    if (!this.#repository.cacheDrafts) this.announce();
    return this.readDraft();
  }

  readAttachments() {
    return cloneAttachments(this.#draft.attachments);
  }

  writeAttachments(attachments) {
    return this.writeDraft({ attachments });
  }

  bindDraft() {
    let expectedAttachmentRevision = this.#draft.attachmentRevision;
    return Object.freeze({
      readDraft: () => this.readDraft().body,
      writeDraft: (body) => this.writeDraft({ body }),
      readAttachments: () => this.readAttachments(),
      writeAttachments: (attachments) => {
        if (this.#draft.attachmentRevision !== expectedAttachmentRevision) return false;
        this.writeAttachments(attachments);
        expectedAttachmentRevision = this.#draft.attachmentRevision;
        return true;
      },
    });
  }

  captureSubmission(message = this.#draft) {
    this.#repository.assertActive();
    assertAddress(this.#identity);
    const operationId = this.#repository.createOperationId();
    const captured = messageSnapshot(this.#repository.contextualize(message));
    const originalDraft = { ...draftSnapshot(this.#draft), body: captured.body, attachments: captured.attachments };
    const call = this.#repository.currentCall();
    this.#draft = {
      ...EMPTY_DRAFT,
      attachments: [],
      revision: this.#draft.revision + 1,
      attachmentRevision: this.#draft.attachmentRevision + 1,
    };
    this.#repository.writeControllerState(this.#identity, { draft: this.#draft });
    void this.#cacheDraft?.write({ body: "", attachments: [], owner: this.#draftOwner });
    const submission = Object.freeze({
      operationId,
      address: publicIdentity(this.#identity),
      choiceRevision: this.#choices.read().revision,
      message: captured,
      originalDraft: Object.freeze(originalDraft),
      clearedDraftRevision: this.#draft.revision,
      scopeEpoch: this.#repository.epoch,
      threadPostOperations: this.#repository.threadPostOperations(),
      call,
    });
    this.#operations.set(operationId, { submission, status: "sending", error: "" });
    this.announce();
    return submission;
  }

  /** Capture a send whose agent does not exist yet. Its call, operation id,
   * creation choice, and provisional owner are fixed now; only the durable
   * triple made by agent.add may complete it later. */
  captureProvisionalSubmission(message = this.#draft, creationChoice = {}) {
    this.#repository.assertActive();
    if (this.#bound) throw new Error("Only an unresolved chat controller can capture a creation send");
    const operationId = this.#repository.createOperationId();
    const captured = messageSnapshot(this.#repository.contextualize(message));
    const originalDraft = { ...draftSnapshot(this.#draft), body: captured.body, attachments: captured.attachments };
    const call = this.#repository.currentCall();
    this.#draft = {
      ...EMPTY_DRAFT,
      attachments: [],
      revision: this.#draft.revision + 1,
      attachmentRevision: this.#draft.attachmentRevision + 1,
    };
    this.#repository.writeControllerState(this.#identity, { draft: this.#draft });
    void this.#cacheDraft?.write({ body: "", attachments: [], owner: this.#draftOwner });
    const submission = Object.freeze({
      operationId,
      creationId: `creation:${operationId}`,
      provisionalDraftId: this.#identity.draftId,
      creationChoice: Object.freeze({ ...creationChoice }),
      message: captured,
      originalDraft: Object.freeze(originalDraft),
      clearedDraftRevision: this.#draft.revision,
      scopeEpoch: this.#repository.epoch,
      threadPostOperations: this.#repository.threadPostOperations(),
      call,
    });
    this.#operations.set(operationId, { submission, status: "creating", error: "" });
    this.announce();
    return submission;
  }

  addressSubmission(submission, call = submission.call) {
    this.#repository.assertSubmissionActive(submission);
    assertAddress(this.#identity);
    if (submission.provisionalDraftId !== this.#identity.draftId && this.#identity.draftId) {
      throw new Error("Creation submission belongs to a different provisional draft");
    }
    return Object.freeze({
      ...submission,
      address: publicIdentity(this.#identity),
      choiceRevision: this.#choices.read().revision,
      call,
    });
  }

  async post(submission, extra = {}) {
    this.#repository.assertSubmissionActive(submission);
    return this.postWithCall(submission.call, submission, extra);
  }

  async retry(submission, extra = {}) {
    this.#repository.assertSubmissionActive(submission);
    return this.postWithCall(this.#repository.currentCall(), submission, extra);
  }

  async postWithCall(call, submission, extra) {
    const { entityId, agentId, conversationId } = submission.address;
    const tracked = this.#operations.get(submission.operationId);
    if (tracked) tracked.extra = { ...extra };
    let receipt;
    try {
      receipt = await call("thread.post", {
        ...submission.message,
        ...extra,
        entity_id: entityId,
        agent_id: agentId,
        conversation_id: conversationId,
        choice_revision: submission.choiceRevision,
        operation_id: submission.operationId,
      });
    } catch (error) {
      this.recordOperationFailure(submission, error);
      throw error;
    }
    this.#repository.assertSubmissionActive(submission);
    try {
      const validated = validatePostReceipt(receipt, submission);
      this.#repository.clearSentSelection(submission.message.viewing_context);
      if (receipt.operation_error) {
        this.#operations.set(submission.operationId, {
          submission,
          status: "execution_error",
          error: receipt.operation_error,
        });
      } else if (receipt.operation_status === "uncertain") {
        this.#operations.set(submission.operationId, { submission, status: "uncertain", error: "" });
      } else {
        this.#operations.delete(submission.operationId);
      }
      this.announce();
      return validated;
    } catch (error) {
      error.uncertain = true;
      this.recordOperationFailure(submission, error);
      throw error;
    }
  }

  recordOperationFailure(submission, error) {
    const previous = this.#operations.get(submission.operationId);
    this.#operations.set(submission.operationId, {
      ...previous,
      submission,
      status: deliveryMayHaveStarted(error) ? "uncertain" : "rejected",
      error: error?.message || String(error),
    });
    this.announce();
  }

  markOperationKind(submission, kind) {
    const operation = this.#operations.get(submission.operationId);
    if (!operation) throw new Error("Unknown chat operation");
    this.#operations.set(submission.operationId, { ...operation, kind });
  }

  clearOperationKind(submission) {
    const operation = this.#operations.get(submission.operationId);
    if (!operation) return;
    this.#operations.set(submission.operationId, { ...operation, kind: "message" });
  }

  async operationStatus(submission) {
    this.#repository.assertSubmissionActive(submission);
    if (!submission.threadPostOperations) throw new Error("Operation status is unavailable on this bridge");
    const status = await this.#repository.currentCall()(submission.threadPostOperations.statusMethod, {
      entity_id: submission.address.entityId,
      agent_id: submission.address.agentId,
      operation_id: submission.operationId,
    });
    this.#repository.assertSubmissionActive(submission);
    validateStatusReceipt(status, submission);
    const operationStatus = status.status;
    if (status.operation_error) {
      this.#operations.set(submission.operationId, {
        submission,
        status: "execution_error",
        error: status.operation_error,
      });
    } else if (operationStatus === "uncertain") {
      this.#operations.set(submission.operationId, { submission, status: operationStatus, error: "" });
    } else {
      this.#operations.delete(submission.operationId);
    }
    this.announce();
    return status;
  }

  async reconcileUncertain() {
    const uncertain = this.#uncertainPosts();
    return Promise.allSettled(uncertain.map((operation) => this.operationStatus(operation.submission)));
  }

  /** The posts this conversation is unsure about, oldest first — insertion
   *  order is submission order, which is the order they have to be settled in. */
  #uncertainPosts() {
    return [...this.#operations.values()].filter((operation) =>
      operation.status === "uncertain" && operation.submission.threadPostOperations,
    );
  }

  /**
   * Settle every uncertain post in this conversation, without a press (#30).
   *
   * One at a time and in submission order: the answers are not independent —
   * re-sending the second message of a pair before the first would land them
   * out of order in the transcript, and a conversation is a sequence.
   *
   * Each answer is a resolution rather than a throw. A post that cannot be
   * settled now stays uncertain and is tried again on the next reconnect, and
   * it does not take the posts behind it down with it.
   */
  async resolveUncertain() {
    const resolved = [];
    for (const operation of this.#uncertainPosts()) {
      resolved.push(await this.#resolveOne(operation));
    }
    return resolved;
  }

  async #resolveOne(operation) {
    const { operationId } = operation.submission;
    try {
      const status = await this.operationStatus(operation.submission);
      // The bridge has it. "uncertain" there is the bridge's own doubt about
      // whether the provider took the turn, which a re-send would answer by
      // writing the message twice — so it stays as it is and keeps its button.
      return status.status === "uncertain"
        ? resolution(operationId, "uncertain-at-bridge")
        : resolution(operationId, "landed", { status: status.status });
    } catch (error) {
      if (!neverReachedTheBridge(error)) return resolution(operationId, "unresolved", { error: error?.message || "" });
      return this.#resendOnce(operation);
    }
  }

  /** The post never arrived, so send it again — once. A re-send that dies on
   *  the wire leaves the post uncertain, and a second automatic attempt every
   *  time the device reconnects is a message the reader never asked to send
   *  four times. After one, it is theirs to retry. */
  async #resendOnce(operation) {
    const { operationId } = operation.submission;
    if (this.#resent.has(operationId)) return resolution(operationId, "given-up");
    this.#resent.add(operationId);
    try {
      await this.retry(operation.submission, operation.extra || {});
      return resolution(operationId, "resent");
    } catch (error) {
      return resolution(operationId, "resend-failed", { error: error?.message || "" });
    }
  }

  recoveries() {
    return [...this.#operations.values()]
      .filter((operation) => ["uncertain", "rejected", "execution_error"].includes(operation.status))
      .map((operation) => ({
        operationId: operation.submission.operationId,
        status: operation.status,
        error: operation.error,
        kind: operation.kind || "message",
        body: operation.submission.message?.body || "",
        attachmentCount: operation.submission.message?.attachments?.length || 0,
        canRetry: operation.kind === "creation" || operation.status === "rejected"
          || (operation.status === "uncertain" && !!operation.submission.threadPostOperations),
      }));
  }

  async retryOperation(operationId, { retryCreation } = {}) {
    const operation = this.#operations.get(operationId);
    if (!operation) throw new Error("Unknown chat operation");
    if (operation.kind === "creation") {
      if (!retryCreation) throw new Error("Agent creation retry is unavailable in this view");
      return retryCreation(operation.submission);
    }
    if (operation.status === "uncertain" && operation.submission.threadPostOperations) {
      return this.operationStatus(operation.submission);
    }
    if (operation.status === "uncertain" && !operation.submission.threadPostOperations) {
      throw new Error("This bridge cannot safely retry an uncertain message");
    }
    const receipt = await this.retry(operation.submission, operation.extra || {});
    this.#failures = this.#failures.filter((failure) => failure.operationId !== operationId);
    this.announce();
    return receipt;
  }

  restoreRejected(submission, error) {
    this.#repository.assertSubmissionActive(submission);
    const noNewerDraft = this.#draft.revision === submission.clearedDraftRevision;
    if (noNewerDraft) {
      this.#operations.delete(submission.operationId);
      this.#draft = {
        body: submission.originalDraft.body,
        attachments: cloneAttachments(submission.originalDraft.attachments),
        revision: this.#draft.revision + 1,
        attachmentRevision: this.#draft.attachmentRevision + 1,
      };
      this.#repository.writeControllerState(this.#identity, { draft: this.#draft });
      this.#cacheDraft?.schedule({ body: this.#draft.body, attachments: storedAttachments(this.#draft.attachments), owner: this.#draftOwner });
      this.announce();
      return "restored";
    }
    this.#failures.push(Object.freeze({
      operationId: submission.operationId,
      address: submission.address,
      choiceRevision: submission.choiceRevision,
      message: submission.message,
      error: (error && error.message) || String(error),
    }));
    this.announce();
    return "retained";
  }

  retryableFailures() {
    return [...this.#failures];
  }

  choice() {
    return this.#choices.read();
  }

  absorbAgent(agent = {}) {
    return this.#choices.absorb(agent);
  }

  requestChoice(next) {
    return this.#choices.request(next);
  }

  setProvisionalChoice(next) {
    if (!this.#identity.draftId) throw new Error("Only a provisional chat controller owns a creation choice");
    return this.#choices.setProvisional(next);
  }

  acknowledgeChoice(intent, answer) {
    return this.#choices.acknowledge(intent, answer);
  }

  chooseModel(next) {
    return this.#choices.choose(next);
  }

  subscribe(listener) {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  announce() {
    for (const listener of [...this.#listeners]) listener(this);
  }

  dispose() {
    this.#cacheDraft?.dispose();
    this.#listeners.clear();
  }

  belongsTo(repository) {
    return this.#repository === repository;
  }

  isBound() {
    return this.#bound;
  }

  bindIdentity(identity) {
    const previous = this.#identity;
    this.#cacheDraft?.dispose();
    this.#identity = { ...identity, draftId: this.#identity.draftId };
    this.#bound = true;
    this.#repository.moveControllerState(previous, this.#identity);
    this.#startDraftCache();
    void this.#cacheDraft?.write({ body: this.#draft.body, attachments: storedAttachments(this.#draft.attachments), owner: this.#draftOwner });
  }

  adoptThreadState(threadState) {
    this.#threadState = threadState;
  }
}

export function createChatRepository({
  scope,
  call,
  createOperationId = randomOperationId,
  viewingContext = null,
  storage: providedStorage,
} = {}) {
  if (typeof call !== "function") throw new Error("Chat repository requires an RPC call function");
  let active = true;
  let currentCall = call;
  const scopeKey = scopeKeyOf(scope);
  const storage = storageOrNull(providedStorage);
  const localState = readLocalState(storage, scopeKey);
  localState.controllers = recordOrEmpty(localState.controllers);
  localState.railViews = recordOrEmpty(localState.railViews);
  // Re-read before every mutation. Another tab may have written a different
  // conversation since this repository was created; a targeted update must
  // preserve that newer, unrelated state without making live UI cross-tab.
  const mutateStoredState = (mutate) => {
    const fresh = readLocalState(storage, scopeKey);
    fresh.controllers = recordOrEmpty(fresh.controllers);
    fresh.railViews = recordOrEmpty(fresh.railViews);
    mutate(fresh);
    writeLocalState(storage, scopeKey, fresh);
    localState.controllers = fresh.controllers;
    localState.railViews = fresh.railViews;
  };
  const controllers = new Map();
  const histories = new Map();
  const railViews = new Map();
  let provisionalSequence = 0;
  let epoch = 1;
  let threadPostOperations = null;
  let messageContext = false;
  const optimisticStore = createOptimisticStore();

  const repository = {
    cacheDrafts: providedStorage === undefined,
    draftAddress(identity) {
      return uiAddress({
        deviceId: scope?.deviceId || "",
        entityId: identity.conversationId || identity.entityId || "",
        view: "chat",
        kind: "draft",
        sub: storedControllerKey(identity),
      });
    },
    get epoch() {
      return epoch;
    },

    get scopeKey() {
      return scopeKey;
    },

    createOperationId,

    readControllerState(identity) {
      return recordOrEmpty(localState.controllers[storedControllerKey(identity)]);
    },

    writeControllerState(identity, patch) {
      const savedPatch = repository.cacheDrafts ? Object.fromEntries(Object.entries(patch).filter(([key]) => key !== "draft")) : patch;
      if (!Object.keys(savedPatch).length) return;
      const key = storedControllerKey(identity);
      mutateStoredState((fresh) => {
        fresh.controllers[key] = { ...recordOrEmpty(fresh.controllers[key]), ...savedPatch };
      });
    },

    moveControllerState(previous, next) {
      const from = storedControllerKey(previous);
      const to = storedControllerKey(next);
      mutateStoredState((fresh) => {
        fresh.controllers[to] = {
          ...recordOrEmpty(fresh.controllers[from]),
          ...recordOrEmpty(fresh.controllers[to]),
        };
        if (from !== to) delete fresh.controllers[from];
      });
    },

    optimisticStore() {
      return optimisticStore;
    },

    assertActive() {
      if (!active) throw new Error("Chat repository scope is no longer active");
    },

    assertSubmissionActive(submission) {
      repository.assertActive();
      if (submission.scopeEpoch !== epoch) throw new Error("Chat repository scope is no longer active");
    },

    currentCall() {
      repository.assertActive();
      return currentCall;
    },

    threadPostOperations() {
      return threadPostOperations;
    },

    /// What this message says about where it was written: the reader's position,
    /// and whatever the surface sending it stamped on the message itself — the
    /// workspace a project rail was standing in. The stamp leads, because it is
    /// the standing place the rest was seen from.
    ///
    /// Nothing goes out to a bridge that does not offer `message_context`, a
    /// stamp included: a client sends context where it is taken and nowhere
    /// else.
    contextualize(message = {}) {
      if (!messageContext) return withoutViewingContext(message);
      const context = normalizeViewingContext([
        ...(message.viewing_context?.items || []),
        ...(viewingContext?.snapshot?.()?.items || []),
      ]);
      return context ? { ...message, viewing_context: context } : withoutViewingContext(message);
    },

    clearSentSelection(context) {
      viewingContext?.clearSelectionIfMatches?.(context);
    },

    configureCapabilities(greeting = {}) {
      repository.assertActive();
      const offered = greeting.thread_post_operations;
      threadPostOperations = offered && offered.version === 1 && typeof offered.status_method === "string"
        ? Object.freeze({ version: 1, statusMethod: offered.status_method })
        : null;
      messageContext = greeting.message_context?.version === 1;
      viewingContext?.setEnabled?.(messageContext);
    },

    controller(identity) {
      repository.assertActive();
      assertAddress(identity);
      const key = controllerKey(identity);
      const held = controllers.get(key);
      if (held) {
        if (held.identity.conversationId !== identity.conversationId) {
          throw new Error("Agent controller cannot be rebound to a different conversation");
        }
        return held;
      }
      const controller = new ChatController(repository, identity);
      controllers.set(key, controller);
      return controller;
    },

    createProvisional({ entityId = "", conversationId = "" } = {}) {
      repository.assertActive();
      provisionalSequence += 1;
      const owner = entityId || conversationId || "unaddressed";
      const draftId = `draft-${scopeKey}-${owner}-${provisionalSequence}`;
      const controller = new ChatController(repository, { entityId, conversationId, agentId: "", draftId });
      controllers.set(controllerKey(controller.identity), controller);
      return controller;
    },

    provisional(draftId, { entityId = "", conversationId = "" } = {}) {
      repository.assertActive();
      if (!draftId) throw new Error("Provisional controller requires a draft id");
      const identity = { entityId, conversationId, agentId: "", draftId };
      const key = controllerKey(identity);
      const held = controllers.get(key);
      if (held) return held;
      const controller = new ChatController(repository, identity);
      controllers.set(key, controller);
      return controller;
    },

    resolveProvisional(controller, identity) {
      repository.assertActive();
      if (!controller || !controller.belongsTo(repository) || !controller.identity.draftId) {
        throw new Error("Only this repository's provisional controller can be resolved");
      }
      assertAddress(identity);
      if (controller.isBound()) {
        const same = controller.identity.entityId === identity.entityId
          && controller.identity.agentId === identity.agentId
          && controller.identity.conversationId === identity.conversationId;
        if (same) return controller;
        throw new Error("Provisional chat controller is already bound to another destination");
      }
      const occupied = controllers.get(controllerKey(identity));
      if (occupied && occupied !== controller) throw new Error("Agent already has a chat controller");
      controllers.delete(controllerKey(controller.identity));
      // Keep the provisional id as immutable lineage on the controller. It is
      // no longer used as the repository key, but lets a captured creation
      // submission prove which provisional owner was resolved.
      const provisionalConversationId = controller.identity.conversationId;
      controller.bindIdentity(identity);
      repository.releaseHistory(provisionalConversationId);
      controller.adoptThreadState(repository.history(identity.conversationId).threadState);
      controllers.set(controllerKey(identity), controller);
      return controller;
    },

    historyIdentity(conversationId) {
      const history = repository.history(conversationId);
      return history ? history.identity : null;
    },

    history(conversationId) {
      repository.assertActive();
      if (!conversationId) return null;
      let history = histories.get(conversationId);
      if (!history) {
        const identity = Object.freeze({ scopeKey, conversationId });
        history = Object.freeze({
          identity,
          threadCache: createThreadCache(),
          threadState: createThreadState({ ownerId: identity }),
        });
        histories.set(conversationId, history);
      }
      return history;
    },

    releaseHistory(conversationId) {
      const history = histories.get(conversationId);
      if (!history) return;
      history.threadState.dispose();
      histories.delete(conversationId);
    },

    railView(key) {
      repository.assertActive();
      let state = railViews.get(key);
      if (!state) {
        const saved = localState.railViews[key] || {};
        state = {
          selectedAgentId: typeof saved.selectedAgentId === "string" ? saved.selectedAgentId : null,
          panelMode: saved.panelMode === "console" ? "console" : "chat",
        };
        railViews.set(key, state);
      }
      const persist = () => {
        mutateStoredState((fresh) => {
          fresh.railViews[key] = { ...state };
        });
      };
      return Object.freeze({
        selectedAgentId: () => state.selectedAgentId,
        chooseAgent: (agentId) => { state.selectedAgentId = agentId || null; persist(); },
        panelMode: () => state.panelMode,
        setPanelMode: (mode) => { state.panelMode = mode; persist(); },
      });
    },

    retarget(nextCall) {
      repository.assertActive();
      if (typeof nextCall !== "function") throw new Error("Chat repository requires an RPC call function");
      currentCall = nextCall;
    },

    /**
     * Settle every post this device is unsure about, over the session it is on
     * now (#30 point 2).
     *
     * Called when a reconnect has greeted its bridge, which is the first moment
     * there is anything to ask: the repository outlives a session, so the posts
     * a dead path stranded are still here, and the ledger that can answer them
     * is reachable again. Every conversation, not only the one on screen — the
     * rail reconciles the selected agent when it re-reads its row, and that left
     * a post in an unopened conversation saying "Delivery uncertain" until
     * somebody happened to look at it.
     *
     * Conversations are settled one after another rather than at once: the
     * re-sends are writes to a transcript, and a device that has just come back
     * should not be handed a burst of them.
     *
     * Never throws. A resolution that cannot be reached is reported and tried
     * again on the next reconnect; a repository whose scope has gone, or a
     * bridge with no operation ledger, has nothing to do here and says so with
     * an empty list.
     */
    /**
     * Every attachment fetch a dead path ate is worth asking for again (#30
     * point 3).
     *
     * Each conversation's thread state held its failed fetches apart: a bridge
     * that refused a path stays refused, and one that was never answered because
     * nothing carried it goes back to being asked for. Returns how many paths
     * across how many conversations were released, for the caller's record.
     */
    retryDeferredAttachments() {
      if (!active) return { conversations: 0, paths: 0 };
      let conversations = 0;
      let paths = 0;
      for (const history of histories.values()) {
        const released = history.threadState.retryDeferredAttachments?.() || [];
        if (!released.length) continue;
        conversations += 1;
        paths += released.length;
      }
      return { conversations, paths };
    },

    async resolveUncertainPosts() {
      if (!active || !threadPostOperations) return [];
      const resolved = [];
      for (const controller of [...controllers.values()]) {
        if (!active) break;
        try {
          resolved.push(...await controller.resolveUncertain());
        } catch {
          /* One conversation's failure is not the others': the posts behind it
             are still worth settling, and this one is tried again next time. */
        }
      }
      return resolved;
    },

    dispose() {
      if (!active) return;
      active = false;
      epoch += 1;
      currentCall = null;
      for (const controller of controllers.values()) controller.dispose();
      for (const history of histories.values()) history.threadState.dispose();
      controllers.clear();
      histories.clear();
      railViews.clear();
    },
  };

  return repository;
}
