// Historical project records. Archive entries are intentionally read-only;
// only a retained plan id links back to its review document.

import { esc } from "../core/text.js";

const optionalString = (value) => {
  if (value === null || value === undefined) return null;
  const text = String(value);
  return text.trim() ? text : null;
};

const firstString = (...values) => {
  for (const value of values) {
    const text = optionalString(value);
    if (text !== null) return text;
  }
  return null;
};

const nonNegativeCount = (...values) => {
  for (const value of values) {
    const count = Number(value);
    if (Number.isFinite(count) && count >= 0) return Math.floor(count);
  }
  return 0;
};

const planStageCount = (plan) => {
  for (const stages of [plan.stages, plan.retained_stages, plan.stage_docs, plan.manifest?.stages]) {
    if (Array.isArray(stages)) return stages.length;
  }
  return nonNegativeCount(plan.stage_count, plan.retained_stage_count);
};

const archiveTimestamp = (value) => {
  const timestamp = Date.parse(value || "");
  return Number.isFinite(timestamp) ? timestamp : Number.NEGATIVE_INFINITY;
};

const newestFirst = (left, right) => archiveTimestamp(right.archivedAt) - archiveTimestamp(left.archivedAt);

const normalizePlan = (plan) => ({
  id: optionalString(plan.plan_id),
  goal: firstString(plan.goal, plan.title) || "Untitled plan",
  formerState: firstString(plan.state, plan.former_state) || "unknown",
  archivedAt: optionalString(plan.archived_at),
  stageCount: planStageCount(plan),
});

const normalizeWorktree = (worktree) => {
  return {
    id: optionalString(worktree.worktree_id),
    branch: optionalString(worktree.branch),
    path: optionalString(worktree.path),
    headSha: optionalString(worktree.head_sha),
    upstream: optionalString(worktree.upstream),
    unpushed: worktree.unpushed ?? null,
    dirtyFiles: worktree.dirty_files ?? null,
    action: optionalString(worktree.action) || "archived",
    archivedAt: optionalString(worktree.archived_at),
  };
};

/** Pure: tolerate legacy/malformed buckets and return two newest-first groups. */
export function normalizeArchive(payload = {}) {
  const plans = Array.isArray(payload?.plans) ? payload.plans : [];
  const worktrees = Array.isArray(payload?.worktrees) ? payload.worktrees : [];
  return {
    plans: plans.filter((plan) => plan && typeof plan === "object").map(normalizePlan).sort(newestFirst),
    worktrees: worktrees.filter((worktree) => worktree && typeof worktree === "object").map(normalizeWorktree).sort(newestFirst),
  };
}

const archivedDate = (value) => {
  const timestamp = Date.parse(value || "");
  if (!Number.isFinite(timestamp)) return "Archive date unavailable";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(timestamp));
};

const countLabel = (count, singular) => `${count} ${singular}${count === 1 ? "" : "s"}`;

const planCard = (plan) => {
  const interactive = plan.id !== null;
  const attributes = interactive
    ? `data-plan="${esc(plan.id)}" role="link" tabindex="0"`
    : 'aria-label="Archived plan details unavailable"';
  return `<div class="${interactive ? "card quiet" : "ncard"}" ${attributes}>
    <div class="top"><span class="title">${esc(plan.goal)}</span><span class="chip work">PLAN</span></div>
    <div class="meta"><span>${esc(archivedDate(plan.archivedAt))}</span><span>·</span><span>Former state: ${esc(plan.formerState)}</span><span>·</span><span>${esc(countLabel(plan.stageCount, "stage"))}</span></div>
  </div>`;
};

const scalarMetadata = (value) => {
  if (value === null || value === undefined || typeof value === "object") return null;
  return String(value);
};

const unpushedLabel = (value) => {
  if (value === true) return "unpushed commits";
  if (value === false) return "no unpushed commits";
  const text = scalarMetadata(value);
  return text === null ? null : `${text} unpushed`;
};

const dirtyLabel = (value) => {
  if (value === true) return "dirty";
  if (value === false) return "clean";
  const text = scalarMetadata(value);
  return text === null ? null : `${text} dirty`;
};

const metadataLine = (label, value) => value === null ? "" : `<div class="facts mono">${label}: ${esc(value)}</div>`;

const worktreeCard = (worktree) => {
  const sync = [
    worktree.upstream ? `Upstream ${worktree.upstream}` : null,
    unpushedLabel(worktree.unpushed),
    dirtyLabel(worktree.dirtyFiles),
  ].filter(Boolean).join(" · ");
  const identifier = worktree.id ? `Worktree ${worktree.id}` : "";
  return `<div class="ncard">
    <div class="nhead muted"><span>${esc(worktree.branch || "(detached)")}</span><span class="chip work">${esc(worktree.action)}</span></div>
    <div class="nsub">Finish action: ${esc(worktree.action)} · ${esc(archivedDate(worktree.archivedAt))}</div>
    ${metadataLine("Path", worktree.path)}
    ${metadataLine("HEAD", worktree.headSha)}
    ${sync ? `<div class="facts mono">${esc(sync)}</div>` : ""}
    ${identifier ? `<div class="facts mono">${esc(identifier)}</div>` : ""}
  </div>`;
};

const bucket = (label, items, renderItem, emptyText) => `<section class="bucket">
  <h2>${label}${items.length ? ` <span class="n">${items.length}</span>` : ""}</h2>
  ${items.length ? items.map(renderItem).join("") : `<div class="empty">${emptyText}</div>`}
</section>`;

/** Pure archive body HTML. All response strings are escaped at interpolation. */
export function archiveHtml(payload) {
  const archive = normalizeArchive(payload);
  return `<div class="project-inbox">
    ${bucket("PLANS", archive.plans, planCard, "No archived plans yet.")}
    ${bucket("WORKTREES", archive.worktrees, worktreeCard, "No archived worktrees yet.")}
  </div>`;
}

/** Fetch and quietly refresh a project's archive. Returns { dispose() }. */
export function mountArchiveTab(host, { projectId, callRpc, navigate, pollMs = 10000 }) {
  let disposed = false;
  let hasRendered = false;
  host.innerHTML = '<div class="empty">Loading archive…</div>';

  const draw = (payload) => {
    host.innerHTML = archiveHtml(payload);
    host.querySelectorAll("[data-plan]").forEach((card) => {
      const openPlan = () => navigate({ name: "plan", projectId, id: card.dataset.plan, tab: "conversation" });
      card.onclick = openPlan;
      card.onkeydown = (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        openPlan();
      };
    });
    hasRendered = true;
  };

  const load = async () => {
    try {
      const payload = await callRpc("archive.list", { project_id: projectId });
      if (!disposed) draw(payload || {});
    } catch {
      if (!disposed && !hasRendered) {
        host.innerHTML = '<div class="empty">Archive is temporarily unavailable. It will retry automatically.</div>';
      }
    }
  };

  load();
  const poll = setInterval(load, pollMs);
  return {
    dispose() {
      disposed = true;
      clearInterval(poll);
    },
  };
}
