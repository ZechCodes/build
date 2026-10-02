import { esc } from "./text.js";
import { directoryTabsHtml } from "./workspaceDirectoryTabs.js";

export const reviewSnapshot = (review, id) => review?.snapshots.find((snapshot) => snapshot.id === id) || review?.snapshots.at(-1) || null;
export const reviewDirectory = (snapshot, id) => snapshot?.directories.find((directory) => directory.id === id) || snapshot?.directories[0] || null;
export const defaultReviewView = (directory) => directory?.status === "not_git" ? "files" : "changes";

export function reviewHeadHtml(review, snapshot, support) {
  const completion = review?.completion ? `<p class="task-review-completion">Completed: ${esc(review.completion.description)}</p>` : "";
  const picker = snapshot ? `<label>Snapshot <select data-review-snapshot>${review.snapshots.map((row) =>
    `<option value="${esc(row.id)}"${row.id === snapshot.id ? " selected" : ""}>${row.number} · ${esc(row.created_at)}</option>`).join("")}</select></label>` : "";
  const reviewer = snapshot ? '<button class="btn" data-review-reviewer>Choose reviewer</button>' : "";
  return `<header class="task-review-head"><h2>Review</h2>${picker}${reviewer}${support.get ? '<button class="btn" data-review-refresh>Refresh</button>' : ""}</header>${completion}`;
}

export function reviewDirectoryHtml(snapshot, directory, view) {
  if (!directory) return '<p class="sub">No directories in this snapshot.</p>';
  const tabs = directoryTabsHtml(snapshot.directories.map((row) => ({ sourceId: row.id, label: row.name, current: row.id === directory.id })));
  const viewTabs = ["changes", "files"].map((name) => `<button type="button" class="btn${view === name ? " primary" : ""}" data-review-view="${name}" aria-pressed="${view === name}">${name === "changes" ? "Changes" : "Files"}</button>`).join("");
  return `${tabs}<div class="task-review-facts">${directoryFacts(directory)}</div><div class="task-review-views" role="group" aria-label="Directory view">${viewTabs}</div>`;
}

function directoryFacts(directory) {
  const dirty = directory.uncommitted_files ? `<p>${Number(directory.uncommitted_files)} uncommitted files not in this review</p>` : "";
  if (directory.status === "not_git") return '<p>Not a Git repository</p><p>Live files — not saved with this review</p>';
  if (directory.status === "unavailable") return `<p>Source unavailable</p><p>${esc(directory.reason || "")}</p>`;
  if (directory.status === "no_commits") return `<p>No commits yet</p>${dirty}`;
  return `${gitFacts(directory)}${dirty}`;
}

function gitFacts(directory) {
  const base = directory.base;
  const baseName = base?.name || (base?.kind === "empty_tree" ? "Empty tree" : "Base");
  return `<p>${esc(directory.branch || "Detached HEAD")} · Base ${esc(baseName)} <code>${esc(base?.oid || "")}</code> → Head <code>${esc(directory.head || "")}</code></p>`;
}
