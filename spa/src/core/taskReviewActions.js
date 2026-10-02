// Snapshot-scoped action choices live in build-ui. Git runs only from an
// explicit submit; cached action rows can finish an already submitted review.
import { esc } from "./text.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { reviewFailure } from "./taskReviewCache.js";
import { fieldTraits } from "./fieldTraits.js";

const actionRows = (review, snapshot) => (review?.actions || []).filter((row) => row.snapshot_id === snapshot.id);
const destinations = (review, snapshot) => (review?.destinations || []).filter((row) => row.snapshot_id === snapshot.id);
const options = (values, selected) => values.map((value) => `<option value="${esc(value)}"${value === selected ? " selected" : ""}>${esc(value)}</option>`).join("");
const remoteChoices = (destination, selected) => options((destination.remotes || []).map((remote) => remote.name), selected);
const branchChoices = (destination, remoteName, selected) => options(destination.remotes?.find((remote) => remote.name === remoteName)?.branches || [], selected);
const oid = (head) => head ? `<code>${esc(head)}</code>` : "unknown";
const running = (rows, directoryId) => rows.some((row) => row.directory_id === directoryId && row.status === "running");
const checked = (value) => value ? " checked" : "";
const selectedRemote = (destination, choice) => choice.remote || destination.remotes?.[0]?.name || "";
const selectedPushBranch = (destination, choice, remote) => choice.pushBranch ||
  destination.remotes?.find((row) => row.name === remote)?.branches?.[0] || "";

function sourceFacts(destination, directory) {
  const different = destination.live_head && directory.head && destination.live_head !== directory.head;
  const warning = destination.error ? `<p class="warn">${esc(destination.error)}</p>` : "";
  return `<p>Repository: ${esc(destination.source_path || directory.source_path || "")}</p>
    <p>Saved head: ${oid(directory.head)} · Live head: ${oid(destination.live_head)}${different ? " · differs from saved head" : ""}</p>${warning}`;
}

function sourceFields(destination, directory, choice) {
  const remote = selectedRemote(destination, choice);
  const branch = selectedPushBranch(destination, choice, remote);
  return `<label><input type="checkbox" data-review-merge="${esc(directory.id)}"${checked(choice.merge)}> Merge</label>
    <label>Into branch<select data-review-merge-branch="${esc(directory.id)}">${options(destination.branches || [], choice.mergeBranch)}</select></label>
    <label><input type="checkbox" data-review-push="${esc(directory.id)}"${checked(choice.push)}> Push</label>
    <label>Remote<select data-review-remote="${esc(directory.id)}">${remoteChoices(destination, remote)}</select></label>
    <label>Branch<input data-review-push-branch="${esc(directory.id)}" ${fieldTraits("identifier")} value="${esc(branch)}" list="review-push-branches-${esc(directory.id)}" placeholder="Existing or new branch">
      <datalist id="review-push-branches-${esc(directory.id)}">${branchChoices(destination, remote, branch)}</datalist></label>`;
}

function sourceHtml(destination, directory, choice, rows) {
  const disabled = running(rows, directory.id) || Boolean(destination.error);
  return `<fieldset class="task-review-source" data-review-source="${esc(directory.id)}"${disabled ? " disabled" : ""}>
    <legend>${esc(directory.name)}</legend>${sourceFacts(destination, directory)}${sourceFields(destination, directory, choice)}
  </fieldset>`;
}

function retryable(row) {
  const merge = row.steps?.find((step) => step.kind === "merge" && step.status === "succeeded");
  const push = row.steps?.find((step) => step.kind === "push" && ["failed", "interrupted"].includes(step.status));
  return merge && push?.remote && push?.branch ? push : null;
}

function resultHtml(row, canAct) {
  const steps = (row.steps || []).map((step) => `<li>${esc(step.kind)} ${esc(step.remote ? `${step.remote}/` : "")}${esc(step.branch)}: ${esc(step.status)}${step.error ? ` · ${esc(step.error)}` : ""}${step.warning ? ` · ${esc(step.warning)}` : ""}${step.input_head ? ` · from ${oid(step.input_head)}` : ""}${step.result_head ? ` · result ${oid(step.result_head)}` : ""}</li>`).join("");
  return `<li class="task-review-result"><strong>${esc(row.source_name || row.directory_id)}: ${esc(row.status)}</strong>
    <p>${esc(row.source_path || "")} · ${esc(row.started_at || "")}${row.finished_at ? ` → ${esc(row.finished_at)}` : ""}</p>
    <ul>${steps}</ul>${canAct && retryable(row) ? `<button type="button" class="btn" data-review-retry-push="${esc(row.id)}">Retry Push</button>` : ""}</li>`;
}

function choicesFor(review, snapshot, selected) {
  const rows = actionRows(review, snapshot);
  return destinations(review, snapshot).map((destination) => {
    const directory = snapshot.directories.find((item) => item.id === destination.directory_id);
    return directory ? sourceHtml(destination, directory, selected[directory.id] || {}, rows) : "";
  }).join("");
}

function selectedSource(destination, choice) {
  const source = { directory_id: destination.directory_id };
  if (choice.merge) source.merge = { branch: choice.mergeBranch || destination.branches?.[0] };
  if (choice.push) {
    const remote = selectedRemote(destination, choice);
    source.push = { remote, branch: selectedPushBranch(destination, choice, remote) };
  }
  return source.merge || source.push ? [source] : [];
}

function requestFor(review, snapshot, selected) {
  const rows = actionRows(review, snapshot);
  return destinations(review, snapshot).filter((destination) => !destination.error && !running(rows, destination.directory_id))
    .flatMap((destination) => selectedSource(destination, selected[destination.directory_id] || {}));
}

function requestError(sources) {
  if (!sources.length) return "Choose Merge or Push for at least one source.";
  if (sources.some((source) => source.merge && !source.merge.branch)) return "Choose a Merge branch.";
  if (sources.some((source) => source.push && (!source.push.remote || !source.push.branch))) return "Choose a Push destination.";
  return "";
}

function matchingRows(review, intent) {
  const rows = (review?.actions || []).filter((row) => row.snapshot_id === intent.snapshotId && !intent.before.includes(row.id)
    && row.actor?.kind === "user");
  return intent.sources.map((source) => rows.find((row) => row.directory_id === source.directory_id &&
    row.steps?.length === Number(Boolean(source.merge)) + Number(Boolean(source.push)) &&
    row.steps.every((step, index) => {
      const wanted = index === 0 && source.merge ? { kind: "merge", ...source.merge } : { kind: "push", ...source.push };
      return step.kind === wanted.kind && step.branch === wanted.branch && (step.remote || "") === (wanted.remote || "");
    })));
}

function unresolvedFailures(review, snapshot) {
  const latest = new Map();
  for (const row of actionRows(review, snapshot)) latest.set(row.directory_id, row);
  return [...latest.values()].some((row) => ["failed", "interrupted", "running"].includes(row.status));
}

function completedDescription(rows) {
  return rows.flatMap((row) => row.steps.map((step) => `${step.kind === "merge" ? "merged" : "pushed"} ${row.source_name || row.directory_id} ${step.kind === "merge" ? `to ${step.branch}` : `to ${step.remote}/${step.branch}`}`)).join("; ");
}

export function mountTaskReviewActions(host, options) {
  const { deviceId, projectId, taskId, snapshot, repository, support, onTaskChanged } = options;
  let review = options.review;
  let draft = { selected: {}, intent: null };
  let busy = false;
  let disposed = false;
  let error = "";
  let completingVersion = null;
  const state = watchUiState(uiAddress({ deviceId, entityId: projectId, view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify([taskId, snapshot.id]) }), (saved) => {
    draft = saved || { selected: {}, intent: null };
    paint(); checkCompletion();
  }, { debounceMs: 180 });
  const save = (changes) => { draft = { ...draft, ...changes }; state.schedule(draft); };
  const editChoice = (id, changes) => save({ selected: { ...draft.selected, [id]: { ...draft.selected[id], ...changes } } });
  const setError = (message) => { error = message; paint(); };

  async function act(sources) {
    if (busy || !sources.length) return;
    busy = true;
    setError("");
    const intent = { snapshotId: snapshot.id, sources, before: actionRows(review, snapshot).map((row) => row.id) };
    save({ intent });
    try {
      await state.flush();
      await repository.mutate("act", { expected_version: review.version, snapshot_id: snapshot.id, sources });
    } catch (failure) {
      if ((failure.code || failure.error_code) === "stale_version") save({ intent: null });
      setError(reviewFailure(failure));
    } finally { busy = false; paint(); checkCompletion(); }
  }

  async function complete(rows) {
    if (!support.complete || busy || completingVersion === review.version) return;
    completingVersion = review.version;
    try {
      await repository.mutate("complete", { expected_version: review.version, description: completedDescription(rows) });
      save({ intent: null });
      await state.flush();
      await onTaskChanged?.();
    } catch (failure) { setError(reviewFailure(failure)); }
  }

  function checkCompletion() {
    if (!draft.intent || !review || review.state !== "open") return;
    const rows = matchingRows(review, draft.intent);
    if (rows.some((row) => !row)) return;
    if (rows.some((row) => ["failed", "interrupted"].includes(row.status))) { save({ intent: null }); return; }
    if (unresolvedFailures(review, snapshot)) return;
    if (rows.every((row) => row.status === "succeeded" && row.steps.every((step) => step.status === "succeeded"))) void complete(rows);
  }

  function renderHtml(rows) {
    const wasOpen = host.querySelector('[data-review-act-sheet]')?.open || false;
    const form = support.act && review?.state === "open" ? `<details data-review-act-sheet${wasOpen ? " open" : ""}><summary>Merge and Push</summary>
      <form data-review-act>${choicesFor(review, snapshot, draft.selected)}<button class="btn primary" type="submit"${busy ? " disabled" : ""}>Run selected steps</button></form></details>` : "";
    host.innerHTML = `${form}${rows.length ? `<div class="task-review-results"><h3>Results</h3><ul>${rows.map((row) => resultHtml(row, support.act && review?.state === "open")).join("")}</ul></div>` : ""}
      <p class="warn" data-review-act-error role="alert"${error ? "" : " hidden"}>${esc(error)}</p>`;
  }

  function paint() {
    if (disposed) return;
    const rows = actionRows(review, snapshot);
    renderHtml(rows);
    host.querySelector('[data-review-act]')?.addEventListener("submit", (event) => {
      event.preventDefault();
      const sources = requestFor(review, snapshot, draft.selected);
      const message = requestError(sources);
      if (message) setError(message); else void act(sources);
    });
    host.querySelectorAll('[data-review-source]').forEach((node) => {
      const id = node.dataset.reviewSource;
      const on = (selector, key, eventName = "change") => node.querySelector(selector)?.addEventListener(eventName, (event) => editChoice(id, { [key]: key === "merge" || key === "push" ? event.target.checked : event.target.value }));
      on('[data-review-merge]', "merge"); on('[data-review-push]', "push");
      on('[data-review-merge-branch]', "mergeBranch"); on('[data-review-push-branch]', "pushBranch", "input");
      node.querySelector('[data-review-remote]')?.addEventListener("change", (event) => {
        const remote = event.target.value;
        const destination = destinations(review, snapshot).find((item) => item.directory_id === id);
        editChoice(id, { remote, pushBranch: destination?.remotes.find((item) => item.name === remote)?.branches?.[0] || "" });
        paint();
      });
    });
    host.querySelectorAll('[data-review-retry-push]').forEach((button) => { button.onclick = () => {
      const row = rows.find((item) => item.id === button.dataset.reviewRetryPush);
      const push = row && retryable(row);
      if (push && !running(rows, row.directory_id)) void act([{ directory_id: row.directory_id,
        push: { remote: push.remote, branch: push.branch, merge_action_id: row.id } }]);
    }; });
  }
  paint();
  return { update(nextReview) { review = nextReview; paint(); checkCompletion(); }, dispose() { disposed = true; state.dispose(); } };
}
