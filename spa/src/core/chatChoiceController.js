const revisionOf = (agent) => (Number.isSafeInteger(agent.choice_revision) ? agent.choice_revision : 0);

const choiceValue = (pending, acknowledged, field, settled) => {
  if (pending && !acknowledged) return pending[field];
  return settled || "";
};

const digestAcknowledges = (pending, agent, revision, dispatched) =>
  !!pending && dispatched.has(pending.sequence) && revision > pending.expectedRevision
  && (agent.model || "") === pending.model && (agent.effort || "") === pending.effort;

function validateAcknowledgement(answer, identity) {
  if (!answer || answer.agent_id !== identity.agentId) {
    throw new Error("agent.choose returned an acknowledgement for a different agent");
  }
  if (answer.entity_id && answer.entity_id !== identity.entityId) {
    throw new Error("agent.choose returned an acknowledgement for a different entity");
  }
  if (!Number.isSafeInteger(answer.choice_revision) || answer.choice_revision < 0) {
    throw new Error("agent.choose returned an invalid choice revision");
  }
}

const digestBelongsTo = (agent, identity) => !agent.id || !identity.agentId || agent.id === identity.agentId;

function stateFromDigest(current, agent, revision, acknowledged) {
  const pending = current.pendingIntent;
  return {
    provider: agent.provider || current.provider,
    requestedModel: choiceValue(pending, acknowledged, "model", agent.model),
    settledModel: agent.model || "",
    activeModel: agent.active_model || "",
    effort: choiceValue(pending, acknowledged, "effort", agent.effort),
    settledEffort: agent.effort || "",
    revision,
    pendingIntent: acknowledged ? null : pending,
  };
}

export function createChatChoiceController({ repository, identityOf, announce }) {
  let sequence = 0;
  let strictRevision = false;
  let tail = null;
  const dispatched = new Set();
  let state = {
    provider: "",
    requestedModel: "",
    settledModel: "",
    activeModel: "",
    effort: "",
    settledEffort: "",
    revision: 0,
    pendingIntent: null,
  };

  const read = () => ({
    provider: state.provider,
    requestedModel: state.requestedModel,
    activeModel: state.activeModel,
    effort: state.effort,
    revision: state.revision,
    pending: !!state.pendingIntent,
  });

  const absorb = (agent = {}) => {
    const identity = identityOf();
    if (!digestBelongsTo(agent, identity)) return false;
    const revision = revisionOf(agent);
    if (revision < state.revision) return false;
    if (Object.hasOwn(agent, "choice_revision")) strictRevision = true;
    const pending = state.pendingIntent;
    const acknowledged = digestAcknowledges(pending, agent, revision, dispatched);
    state = stateFromDigest(state, agent, revision, acknowledged);
    announce();
    return true;
  };

  const request = ({ model = "", effort = "" }) => {
    repository.assertActive();
    sequence += 1;
    const intent = Object.freeze({
      sequence,
      model,
      effort,
      expectedRevision: state.revision,
      address: identityOf(),
      call: repository.currentCall(),
      scopeEpoch: repository.epoch,
    });
    state = { ...state, requestedModel: model, effort, pendingIntent: intent };
    announce();
    return intent;
  };

  const absorbOlderAck = (answer) => {
    if (answer.choice_revision <= state.revision) return false;
    state = {
      ...state,
      settledModel: answer.model || "",
      settledEffort: answer.effort || "",
      revision: answer.choice_revision,
    };
    return false;
  };

  const rejectStaleAck = (current, intent) => {
    if (!current || intent.sequence !== current.sequence) return false;
    state = {
      ...state,
      requestedModel: state.settledModel,
      effort: state.settledEffort,
      pendingIntent: null,
    };
    announce();
    return false;
  };

  const acknowledge = (intent, answer) => {
    repository.assertSubmissionActive(intent);
    validateAcknowledgement(answer, identityOf());
    dispatched.delete(intent.sequence);
    const current = state.pendingIntent;
    if (answer.choice_revision < state.revision) return rejectStaleAck(current, intent);
    if (!current || intent.sequence !== current.sequence) return absorbOlderAck(answer);
    state = {
      ...state,
      provider: answer.provider || state.provider,
      requestedModel: answer.model || "",
      settledModel: answer.model || "",
      effort: answer.effort || "",
      settledEffort: answer.effort || "",
      revision: answer.choice_revision,
      pendingIntent: null,
    };
    announce();
    return true;
  };

  const legacyAcknowledgement = (intent, answered) => ({
    ...(answered || {}),
    entity_id: intent.address.entityId,
    agent_id: intent.address.agentId,
    model: intent.model,
    effort: intent.effort,
    choice_revision: state.revision + 1,
  });

  const send = async (intent) => {
    repository.assertSubmissionActive(intent);
    dispatched.add(intent.sequence);
    const answered = await intent.call("agent.choose", {
      entity_id: intent.address.entityId,
      agent_id: intent.address.agentId,
      model: intent.model,
      effort: intent.effort,
      expected_choice_revision: state.revision,
    });
    repository.assertSubmissionActive(intent);
    const answer = answered?.agent_id ? answered : strictRevision ? answered : legacyAcknowledgement(intent, answered);
    return acknowledge(intent, answer);
  };

  const reject = (intent) => {
    dispatched.delete(intent.sequence);
    if (!state.pendingIntent || intent.sequence !== state.pendingIntent.sequence) return false;
    state = { ...state, requestedModel: state.settledModel, effort: state.settledEffort, pendingIntent: null };
    announce();
    return true;
  };

  const choose = (next) => {
    const intent = request(next);
    const sendIntent = () => send(intent);
    const choosing = tail ? tail.catch(() => {}).then(sendIntent) : sendIntent();
    tail = choosing;
    return choosing.finally(() => {
      if (tail === choosing) tail = null;
    }).catch((error) => {
      reject(intent);
      throw error;
    });
  };

  return Object.freeze({
    read,
    absorb,
    request,
    acknowledge,
    choose,
    setProvisional({ provider = "", model = "", effort = "" }) {
      state = { ...state, provider, requestedModel: model, effort };
      announce();
    },
  });
}
