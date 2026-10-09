// PR integration is one durable server plan. Local drafts never replay Git;
// the caller supplies cached review records and RPC answers never paint here.
import "./taskReviewMerge.css";
import { esc } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";
import { watchReviewActionDraft } from "./taskReviewDrafts.js";
import { reviewFailure } from "./taskReviewCache.js";
import { reviewSupportFor } from "./taskReviewSupport.js";

const activeStatus = (review) => ["open", "changes_requested", "approved"].includes(review?.pull_request?.status);
const fullHead = (value) => /^[a-f\d]{40}$/i.test(value || "");
const shortBranch = (value = "") => value.replace(/^refs\/heads\//, "");
const code = (value) => `<code>${esc(value || "Unavailable")}</code>`;
const checked = (value) => value ? " checked" : "";
const disabled = (value) => value ? " disabled" : "";
const warningHtml = (message) => message ? `<p class="warn">${esc(message)}</p>` : "";
const intentsOf = (record) => record?.merge_intents || [];
const rowsFor = (review, intent) => (review?.actions || []).filter((row) => intent.action_ids?.includes(row.id));
const sourceRows = (review, intent, source) => rowsFor(review, intent).filter((row) =>
  row.directory_id === source.directory_id && row.snapshot_id === intent.request.snapshot_id);

const mergeStepMatches = (step, source) => step.kind === "merge" && step.status === "succeeded" &&
  fullHead(step.result_head) && step.input_head === source.head && step.branch === shortBranch(source.base_branch_ref);
const successfulMerges = (review, intent, source) => sourceRows(review, intent, source).flatMap((row) =>
  (row.steps || []).filter((step) => mergeStepMatches(step, source)).map((step) => ({ actionId: row.id, head: step.result_head })));
const integrated = (review, intent, source) => successfulMerges(review, intent, source).length > 0;

function pushLinked(row, step, merged) {
  if (step.merge_action_id) return step.merge_action_id === merged.actionId;
  return row.id === merged.actionId;
}

const pushMatches = (row, step, merged, push) => step.kind === "push" && step.status === "succeeded" &&
  step.remote === push.remote && step.branch === push.branch && step.input_head === merged.head &&
  step.result_head === merged.head && pushLinked(row, step, merged);

function published(review, intent, source) {
  if (!source.push) return true;
  const merges = successfulMerges(review, intent, source);
  return sourceRows(review, intent, source).some((row) => row.steps?.some((step) =>
    merges.some((merged) => pushMatches(row, step, merged, source.push))));
}

const unpushed = (review, intent) => intent.request.sources.some((source) =>
  integrated(review, intent, source) && !published(review, intent, source));
const uncertain = (review, intent) => rowsFor(review, intent).some((row) => row.status === "running" ||
  row.steps?.some((step) => step.status === "running"));

const sourceInterrupted = (review, intent, source) => sourceRows(review, intent, source).some((row) =>
  row.status === "interrupted" || row.steps?.some((step) => step.status === "interrupted"));
const sourceSettled = (review, intent, source) => integrated(review, intent, source) && published(review, intent, source);

function interruptionPending(review, intent) {
  return intent.request.sources.some((source) => (intent.state === "interrupted" || sourceInterrupted(review, intent, source)) &&
    !sourceSettled(review, intent, source));
}

const bindingsIntegrated = (review, intents) => (review.bindings || []).length > 0 && review.bindings.every((binding) =>
  intents.some((intent) => intent.request.sources.some((source) => source.directory_id === binding.directory_id && integrated(review, intent, source))));

/** The merged PR is authoritative. An old failure before integration does not
 * hold reclaim; running, uncertain work and unpublished saved results do. */
export function settledMerge(record) {
  const review = record?.review;
  const intents = intentsOf(record);
  return review?.pull_request?.status === "merged" && bindingsIntegrated(review, intents) && intents.every((intent) =>
    intent.state !== "running" && !uncertain(review, intent) && !interruptionPending(review, intent) && !unpushed(review, intent));
}

const wireSource = (source) => ({ directory_id: source.directory_id, expected_base_head: source.expected_base_head,
  ...(source.push ? { push: { remote: source.push.remote, branch: source.push.branch } } : {}) });
const retryRequest = (review, intent) => ({ expected_version: review.version, snapshot_id: intent.request.snapshot_id,
  sources: intent.request.sources.map(wireSource) });
const refreshedRequest = (review, record, intent) => ({ ...retryRequest(review, intent),
  sources: intent.request.sources.map((source) => ({ ...wireSource(source),
    expected_base_head: record.sync.find((row) => row.directory_id === source.directory_id).target_head })) });
const destinationFor = (review, snapshot, id) => review.destinations?.find((row) => row.snapshot_id === snapshot.id && row.directory_id === id);
const externalRemotes = (destination, binding) => (destination?.remotes || []).filter((remote) => remote.name !== binding.remote_name);
function choiceFor(draft, destination, binding) {
  const submitted = draft.submitted?.sources.find((source) => source.directory_id === binding.directory_id);
  const choice = { remote: externalRemotes(destination, binding)[0]?.name || "",
    branch: shortBranch(binding.base_branch_ref), ...draft.selected?.[binding.directory_id] };
  return submitted ? { ...choice, push: Boolean(submitted.push), ...submitted.push } : choice;
}

function mergeBlock(review, record, snapshot) {
  if (!activeStatus(review)) return "Merge requires an active PR.";
  if (snapshot.id !== review.pull_request.latest_published_snapshot_id) return "Select the latest published snapshot to merge.";
  if (!review.bindings?.length) return "Included Git sources are unavailable.";
  if (intentsOf(record).some((intent) => intent.state === "running")) return "A merge is already running.";
  return sourceBlock(review, record, snapshot);
}

function sourceBlock(review, record, snapshot) {
  for (const binding of review.bindings) {
    const directory = snapshot.directories.find((row) => row.id === binding.directory_id);
    const observation = record?.sync?.find((row) => row.directory_id === binding.directory_id);
    const error = observationBlock(directory, observation);
    if (error) return error;
  }
  return "";
}

function observationBlock(directory, observation) {
  if (!fullHead(directory?.head) || !fullHead(observation?.target_head)) return "Refresh review to read the saved heads and current merge targets.";
  if (observation.error || !["current", "pending"].includes(observation.health)) return observation.error || "Refresh review to check the merge targets.";
  if (observation.received_head !== directory.head) return "New received work is pending. Refresh review before merging.";
  return "";
}

function branchValid(branch) {
  return Boolean(branch) && new TextEncoder().encode(branch).length <= 1024 &&
    !/(^[-/]|[/.]$|[\s\x00-\x1f\x7f~^:?*\[\\]|\.\.|@\{|\/\/|(^|\/)\.|\.lock(\/|$))/.test(branch);
}

function selectedRequest(review, record, snapshot, draft) {
  return { expected_version: review.version, snapshot_id: snapshot.id, sources: review.bindings.map((binding) => {
    const destination = destinationFor(review, snapshot, binding.directory_id);
    const choice = choiceFor(draft, destination, binding);
    const observation = record.sync.find((row) => row.directory_id === binding.directory_id);
    return { directory_id: binding.directory_id, expected_base_head: observation.target_head,
      ...(choice.push ? { push: { remote: choice.remote, branch: choice.branch } } : {}) };
  }) };
}

function pushError(review, snapshot, request) {
  for (const source of request.sources) {
    if (!source.push) continue;
    const binding = review.bindings.find((row) => row.directory_id === source.directory_id);
    const destination = destinationFor(review, snapshot, source.directory_id);
    if (destination?.error) return destination.error;
    if (!externalRemotes(destination, binding).some((remote) => remote.name === source.push.remote)) return "Choose a configured external Push remote.";
    if (!branchValid(source.push.branch)) return "Choose a valid Push branch.";
  }
  return "";
}

function pushFieldsHtml(binding, destination, choice) {
  const id = binding.directory_id;
  const remotes = externalRemotes(destination, binding);
  const remoteOptions = remotes.map((remote) => `<option value="${esc(remote.name)}"${remote.name === choice.remote ? " selected" : ""}>${esc(remote.name)}</option>`).join("");
  const branches = remotes.find((remote) => remote.name === choice.remote)?.branches || [];
  return `<label><input type="checkbox" data-pr-merge-push="${esc(id)}"${checked(choice.push)}>Push the merged result externally</label>
    <label>Remote<select data-pr-merge-remote="${esc(id)}"${disabled(!choice.push)}>${remoteOptions}</select></label>
    <label>Push branch<input data-pr-merge-branch="${esc(id)}" ${fieldTraits("identifier")} value="${esc(choice.branch)}" list="pr-merge-branches-${esc(id)}"${disabled(!choice.push)}></label>
    <datalist id="pr-merge-branches-${esc(id)}">${branches.map((branch) => `<option value="${esc(branch)}"></option>`).join("")}</datalist>`;
}

function sourceHtml(review, record, snapshot, draft, binding) {
  const id = binding.directory_id;
  const directory = snapshot.directories.find((row) => row.id === id);
  const destination = destinationFor(review, snapshot, id);
  const observation = record?.sync?.find((row) => row.directory_id === id);
  const choice = choiceFor(draft, destination, binding);
  return `<fieldset class="task-review-source" data-pr-merge-source="${esc(id)}"><legend>${esc(directory?.name || id)}</legend>
    <p>Merge saved head ${code(directory?.head)} into ${code(shortBranch(binding.base_branch_ref))}</p>
    <p>Repository: ${esc(binding.source_repository)}</p>${targetHtml(draft, id, observation)}
    ${pushFieldsHtml(binding, destination, choice)}${warningHtml(destination?.error)}
  </fieldset>`;
}

function targetHtml(draft, id, observation) {
  const source = draft.submitted?.sources.find((row) => row.directory_id === id);
  if (!source) return `<p>Expected target: ${code(observation?.target_head)}</p>`;
  return `<p>Saved expected target: ${code(source.expected_base_head)}</p><p>Current target: ${code(observation?.target_head)}</p>`;
}

function mayRetry(review, intent) {
  if (["running", "succeeded"].includes(intent.state) || uncertain(review, intent)) return false;
  if (unpushed(review, intent)) return true;
  return activeStatus(review) && review.pull_request.latest_published_snapshot_id === intent.request.snapshot_id;
}

const publicationRetry = (review, intent) => intent.request.sources.every((source) => integrated(review, intent, source)) && unpushed(review, intent);
const uncertainResults = (review, intent) => rowsFor(review, intent).some((row) =>
  ["running", "interrupted"].includes(row.status) || row.steps?.some((step) => ["running", "interrupted"].includes(step.status)));
const mayPrepare = (review, intent) => activeStatus(review) && ["failed", "interrupted"].includes(intent.state) &&
  review.pull_request.latest_published_snapshot_id === intent.request.snapshot_id && !uncertainResults(review, intent) &&
  intent.request.sources.some((source) => integrated(review, intent, source));

function intentSummary(review, intent) {
  const sources = intent.request.sources;
  const count = sources.filter((source) => integrated(review, intent, source)).length;
  if (intent.state === "running") return "Merge in progress";
  if (count === sources.length) return integratedSummary(review, intent);
  if (count > 0) return `Partially merged: ${count} of ${sources.length} included sources integrated`;
  return intent.state === "interrupted" ? "Merge interrupted" : `Merge ${intent.state}`;
}

function integratedSummary(review, intent) {
  if (unpushed(review, intent)) return "Merged locally; publication failed";
  if (review.pull_request.status !== "merged") return "Sources integrated; PR merge incomplete";
  return "Merged locally · Publication complete";
}

function resultStepHtml(step) {
  return `<li>${esc(step.kind)} ${esc(step.remote ? `${step.remote}/` : "")}${esc(step.branch)}: ${esc(step.status)}
    ${step.input_head ? ` · from ${code(step.input_head)}` : ""}${step.result_head ? ` · result ${code(step.result_head)}` : ""}
    ${step.error ? ` · ${esc(step.error)}` : ""}${step.warning ? ` · ${esc(step.warning)}` : ""}</li>`;
}

function intentHtml(review, intent, support, busy) {
  const rows = rowsFor(review, intent).map((row) => `<li><strong>${esc(row.source_name || row.directory_id)}: ${esc(row.status)}</strong>
    <p>${esc(row.source_path)}</p><ul>${(row.steps || []).map(resultStepHtml).join("")}</ul></li>`).join("");
  const retry = support.merge && mayRetry(review, intent);
  const prepare = support.merge && mayPrepare(review, intent);
  return `<section class="pr-merge-result" data-pr-merge-result="${esc(intent.request_id)}" data-pr-merge-status="${esc(intent.state)}">
    <h3>${esc(intentSummary(review, intent))}</h3><p>Snapshot: ${esc(intent.request.snapshot_id)}</p>
    ${intent.state === "interrupted" ? '<p class="warn">Interrupted: inspect the saved Git results before retrying.</p>' : ""}
    ${intent.error ? `<p class="warn">${esc(intent.error)}</p>` : ""}<ul>${rows}</ul>
    ${retry ? `<button class="btn" type="button" data-pr-merge-retry="${esc(intent.request_id)}"${busy ? " disabled" : ""}>${publicationRetry(review, intent) ? "Retry publication" : "Retry saved merge"}</button>` : ""}
    ${prepare ? `<button class="btn" type="button" data-pr-merge-prepare="${esc(intent.request_id)}"${busy ? " disabled" : ""}>Prepare a new merge plan</button>` : ""}
  </section>`;
}

const freshDraft = () => ({ selected: {}, submitted: null });
const recoveryPlan = (review, record, snapshot) => intentsOf(record).find((intent) =>
  intent.request.snapshot_id === snapshot.id && (intent.state === "interrupted" || uncertain(review, intent) ||
    intent.request.sources.some((source) => integrated(review, intent, source))));
const focusState = (host) => {
  const active = document.activeElement;
  return host.contains(active) ? { index: [...host.querySelectorAll("input,select,button")].indexOf(active), start: active.selectionStart, end: active.selectionEnd } : null;
};
function restoreFocus(host, held) {
  const control = held && host.querySelectorAll("input,select,button")[held.index];
  control?.focus({ preventScroll: true });
  if (control?.setSelectionRange && held.start != null) control.setSelectionRange(held.start, held.end);
}

export function mountTaskReviewMerge(host, options) {
  const { deviceId, projectId, taskId, snapshot, repository, keepReadingPlace = (paint) => paint() } = options;
  let review = options.review;
  let record = options.record;
  let draft = freshDraft();
  let hydrating = true;
  let busy = false;
  let disposed = false;
  let error = "";
  const scope = { deviceId, projectId, taskId, snapshotId: snapshot.id };
  const support = () => reviewSupportFor(review, options.support);
  const preparedPlan = () => intentsOf(record).find((intent) => intent.request_id === draft.preparedRequestId && mayPrepare(review, intent));
  const submittedPlan = () => draft.submitted && intentsOf(record).find((intent) => intent.request_id === draft.submittedRequestId && mayPrepare(review, intent));
  const recoveryDraft = () => preparedPlan() || submittedPlan();
  const submittedRequest = () => ({ ...draft.submitted, expected_version: review.version });
  const writer = watchReviewActionDraft(scope, "merge", (saved) => {
    if (disposed || busy) return;
    draft = saved || freshDraft();
    paint();
  });
  const ready = writer.ready.then(() => { hydrating = false; paint(); });

  function submittedDraft(request, requestId) {
    const saved = { ...draft, submitted: structuredClone(request),
      submittedRequestId: requestId || draft.preparedRequestId || draft.submittedRequestId };
    delete saved.preparedRequestId;
    return saved;
  }

  async function submit(request, requestId) {
    if (disposed || busy || hydrating || !support().merge) return;
    busy = true; error = "";
    draft = submittedDraft(request, requestId);
    paint();
    try {
      await writer.write(draft);
      if (disposed) return;
      // The shared repository commits the result. A cache read drives update.
      await repository.mutate("merge", request);
      if (!disposed) await options.onTaskChanged?.();
    } catch (failure) {
      if (!disposed) error = reviewFailure(failure);
    } finally { busy = false; paint(); }
  }

  async function submitForm(event) {
    event.preventDefault();
    const blocked = mergeBlock(review, record, snapshot);
    if (blocked) { error = blocked; paint(); return; }
    const request = formRequest();
    const invalid = pushError(review, snapshot, request);
    if (invalid) { error = invalid; paint(); return; }
    await submit(request);
  }

  function formRequest() {
    const prepared = preparedPlan();
    if (prepared) return refreshedRequest(review, record, prepared);
    return draft.submitted ? submittedRequest() : selectedRequest(review, record, snapshot, draft);
  }

  function wireChoices() {
    host.querySelectorAll("[data-pr-merge-source]").forEach((node) => {
      const id = node.dataset.prMergeSource;
      const edit = (changes, repaint = false) => {
        draft = { ...draft, selected: { ...draft.selected, [id]: { ...draft.selected[id], ...changes } } };
        writer.schedule(draft);
        if (repaint) paint();
      };
      node.querySelector("[data-pr-merge-push]").onchange = (event) => edit({ push: event.target.checked }, true);
      node.querySelector("[data-pr-merge-remote]").onchange = (event) => edit({ remote: event.target.value }, true);
      node.querySelector("[data-pr-merge-branch]").oninput = (event) => edit({ branch: event.target.value });
    });
  }

  function formHtml() {
    if (!support().merge || !activeStatus(review)) return "";
    if (recoveryPlan(review, record, snapshot) && !recoveryDraft()) return '<p class="sub">This snapshot has retained Git results. Review its saved merge below or prepare a new merge plan.</p>';
    const blocked = mergeBlock(review, record, snapshot);
    return mergeFormHtml(blocked);
  }

  function mergeFormHtml(blocked) {
    const locked = busy || hydrating;
    const prepared = preparedPlan();
    return `<details data-pr-merge-sheet><summary>Merge PR</summary><form data-pr-merge-form>
      <p>Merge every included Git source into its configured base. Only the selected published heads are included.</p>
      <fieldset class="pr-merge-fields"${disabled(locked || draft.submitted)}>
        ${review.bindings.map((binding) => sourceHtml(review, record, snapshot, draft, binding)).join("")}</fieldset>
      ${warningHtml(blocked)}
      ${prepared ? '<p class="sub">Recorded successful merges and requested Push destinations are retained. Confirm the current targets to resume unfinished work.</p>' : savedPlanHtml()}
      <button class="btn primary" data-pr-merge-submit type="submit"${disabled(locked || blocked)}>${busy ? "Merging…" : draft.submitted ? "Retry saved merge" : "Merge PR"}</button>
      ${draft.submitted && !recoveryDraft() ? `<button class="btn" type="button" data-pr-merge-new${disabled(locked)}>Prepare a new merge plan</button>` : ""}
    </form></details>`;
  }

  function savedPlanHtml() {
    return draft.submitted ? '<p class="sub">This saved merge keeps its original heads and Push destinations. Retry is explicit.</p>' : "";
  }

  function wireRecovery() {
    host.querySelectorAll("[data-pr-merge-prepare]").forEach((button) => {
      button.onclick = async () => {
        const intent = intentsOf(record).find((row) => row.request_id === button.dataset.prMergePrepare);
        if (disposed || busy || hydrating || !intent || !mayPrepare(review, intent)) return;
        draft = { ...draft, submitted: retryRequest(review, intent), preparedRequestId: intent.request_id };
        delete draft.submittedRequestId;
        error = "";
        await writer.write(draft);
        if (disposed) return;
        paint();
        const sheet = host.querySelector("[data-pr-merge-sheet]");
        if (sheet) { sheet.open = true; sheet.scrollIntoView?.({ block: "nearest" }); sheet.querySelector("summary").focus({ preventScroll: true }); }
      };
    });
  }

  function paint() {
    if (disposed) return;
    const wasOpen = host.querySelector("[data-pr-merge-sheet]")?.open;
    const heldFocus = focusState(host);
    keepReadingPlace(() => {
      host.innerHTML = `${formHtml()}${intentsOf(record).map((intent) => intentHtml(review, intent, support(), busy)).join("")}
        <p class="warn" data-pr-merge-error role="alert"${error ? "" : " hidden"}>${esc(error)}</p>`;
      const sheet = host.querySelector("[data-pr-merge-sheet]");
      if (sheet) sheet.open = Boolean(wasOpen);
      restoreFocus(host, heldFocus);
    });
    const form = host.querySelector("[data-pr-merge-form]");
    if (form) { form.onsubmit = submitForm; wireChoices(); }
    const fresh = host.querySelector("[data-pr-merge-new]");
    if (fresh) fresh.onclick = async () => {
      if (busy || hydrating || recoveryPlan(review, record, snapshot)) return;
      draft = { ...draft, submitted: null };
      delete draft.preparedRequestId;
      delete draft.submittedRequestId;
      error = "";
      await writer.write(draft);
      paint();
    };
    host.querySelectorAll("[data-pr-merge-retry]").forEach((button) => {
      button.onclick = () => {
        const intent = intentsOf(record).find((row) => row.request_id === button.dataset.prMergeRetry);
        if (intent && mayRetry(review, intent)) return submit(requestForIntent(intent), intent.request_id);
      };
    });
    wireRecovery();
  }
  function requestForIntent(intent) {
    return draft.submitted && draft.submittedRequestId === intent.request_id ? submittedRequest() : retryRequest(review, intent);
  }
  paint();
  return { ready, update(nextReview, nextRecord) { if (disposed) return; review = nextReview; record = nextRecord; paint(); },
    dispose() { disposed = true; writer.dispose(); } };
}
