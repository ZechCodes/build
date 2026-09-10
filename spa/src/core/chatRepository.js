import { createThreadCache, createThreadState } from "./thread.js";
import { createOptimisticStore } from "./optimistic.js";
import { createChatChoiceController } from "./chatChoiceController.js";

const EMPTY_DRAFT = Object.freeze({ body: "", attachments: [] });
const OPERATION_STATES = new Set(["queued", "claimed", "delivered", "uncertain"]);
const REQUIRED_OPERATION_RECEIPT_FIELDS = [
  "operation_id",
  "entity_id",
  "agent_id",
  "conversation_id",
  "posted_sequence",
  "operation_status",
];

const cloneAttachments = (attachments = []) => attachments.map((attachment) => ({ ...attachment }));
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
  #threadState;

  constructor(repository, identity) {
    this.#repository = repository;
    this.#identity = { ...identity };
    this.#bound = !identity.draftId;
    this.#draft = { ...EMPTY_DRAFT, attachments: [], revision: 0, attachmentRevision: 0 };
    this.#failures = [];
    this.#listeners = new Set();
    this.#threadState = repository.history(identity.conversationId)?.threadState || createThreadState();
    this.#choices = createChatChoiceController({
      repository,
      identityOf: () => publicIdentity(this.#identity),
      announce: () => this.announce(),
    });
    this.#operations = new Map();
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
    this.announce();
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
    const uncertain = [...this.#operations.values()].filter((operation) =>
      operation.status === "uncertain" && operation.submission.threadPostOperations,
    );
    return Promise.allSettled(uncertain.map((operation) => this.operationStatus(operation.submission)));
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
    this.#listeners.clear();
  }

  belongsTo(repository) {
    return this.#repository === repository;
  }

  isBound() {
    return this.#bound;
  }

  bindIdentity(identity) {
    this.#identity = { ...identity, draftId: this.#identity.draftId };
    this.#bound = true;
  }

  adoptThreadState(threadState) {
    this.#threadState = threadState;
  }
}

export function createChatRepository({ scope, call, createOperationId = randomOperationId, viewingContext = null }) {
  if (typeof call !== "function") throw new Error("Chat repository requires an RPC call function");
  let active = true;
  let currentCall = call;
  const scopeKey = scopeKeyOf(scope);
  const controllers = new Map();
  const histories = new Map();
  const railViews = new Map();
  let provisionalSequence = 0;
  let epoch = 1;
  let threadPostOperations = null;
  let messageContext = false;
  const optimisticStore = createOptimisticStore();

  const repository = {
    get epoch() {
      return epoch;
    },

    get scopeKey() {
      return scopeKey;
    },

    createOperationId,

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

    contextualize(message = {}) {
      const context = viewingContext?.snapshot?.();
      if (!context?.items?.length) return message;
      if (!messageContext) return message;
      return { ...message, viewing_context: context };
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
      const draftId = `draft-${scopeKey}-${provisionalSequence}`;
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
        state = { selectedAgentId: null, panelMode: "chat" };
        railViews.set(key, state);
      }
      return Object.freeze({
        selectedAgentId: () => state.selectedAgentId,
        chooseAgent: (agentId) => { state.selectedAgentId = agentId || null; },
        panelMode: () => state.panelMode,
        setPanelMode: (mode) => { state.panelMode = mode; },
      });
    },

    retarget(nextCall) {
      repository.assertActive();
      if (typeof nextCall !== "function") throw new Error("Chat repository requires an RPC call function");
      currentCall = nextCall;
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
