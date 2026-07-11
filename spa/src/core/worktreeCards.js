// The understated board card for a git worktree Build did not create. Every
// field (branch, subject, path) is UNTRUSTED external text and passes through
// esc() at each interpolation.

import { esc, humanAge } from "./text.js";

/** The understated board card for one external worktree (task.list entry). */
export function externalWorktreeCard(w) {
  const stat = w.diffstat || { files_changed: 0, insertions: 0, deletions: 0 };
  const dirty = w.dirty_files ? ` · ${w.dirty_files} uncommitted` : "";
  return `
    <div class="card quiet external" data-wt="${esc(w.worktree_id)}" data-project="${esc(w.project_id)}">
      <div class="top"><span class="title">${esc(w.branch || "(detached)")}</span>
        <span class="chip">WORKTREE</span></div>
      <div class="meta"><span>${esc(w.project)}</span><span>·</span><span>${esc(w.head_subject)}</span></div>
      <div class="payload">${stat.files_changed} files +${stat.insertions} −${stat.deletions}${dirty} · ${humanAge(w.head_age_seconds)}</div>
    </div>`;
}
