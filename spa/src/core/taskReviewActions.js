// Legacy Git results remain readable; new integration uses PR review controls.
import { esc } from "./text.js";
import { mountTaskReviewMerge } from "./taskReviewMerge.js";

function resultHtml(row) {
  const oid = (head) => head ? `<code>${esc(head)}</code>` : "unknown";
  const steps = (row.steps || []).map((step) => `<li>${esc(step.kind)} ${esc(step.remote ? `${step.remote}/` : "")}${esc(step.branch)}: ${esc(step.status)}${step.error ? ` · ${esc(step.error)}` : ""}${step.warning ? ` · ${esc(step.warning)}` : ""}${step.input_head ? ` · from ${oid(step.input_head)}` : ""}${step.result_head ? ` · result ${oid(step.result_head)}` : ""}</li>`).join("");
  return `<li class="task-review-result" data-review-result-status="${esc(row.status)}"><strong>${esc(row.source_name || row.directory_id)}: ${esc(row.status)}</strong><p>${esc(row.source_path || "")} · ${esc(row.started_at || "")}${row.finished_at ? ` → ${esc(row.finished_at)}` : ""}</p><ul>${steps}</ul></li>`;
}

export function mountTaskReviewActions(host, options) {
  if (options.review?.mode === "pull_request") return mountTaskReviewMerge(host, options);
  function paint(review) {
    const rows = (review?.actions || []).filter((row) => row.snapshot_id === options.snapshot.id);
    host.innerHTML = rows.length ? `<div class="task-review-results"><h3>Results</h3><ul>${rows.map(resultHtml).join("")}</ul></div>` : "";
  }
  paint(options.review);
  return { update: paint, dispose() {} };
}
