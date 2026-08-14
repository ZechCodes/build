// Review prioritization, as pure functions: a triage pass joined onto the diff
// the reviewer has open, turned into the order that diff renders in.
//
// The rules are the UX Redesign Decisions doc's "Review prioritization" bullets.
// Triage is an OVERLAY: it orders and collapses, it never filters. Every hunk
// the diff contains is in the plan that comes out of here, in exactly one
// section, whether the pass named it or not — the diff is ground truth and the
// overlay is a reading of it.
//
// - critical hunks pull their file to the top, carrying the pass's rationale;
// - low hunks whose whole file is low collapse into their named group;
// - a hunk the pass did not name renders normally, marked untriaged;
// - the reviewer's own override beats the pass on the hunk it names;
// - a pass the diff has moved under still orders, labelled stale;
// - no pass (or a pass about some other changeset) is the plain stack.
//
// Nothing here touches the DOM or the wire. core/diffRender.js draws the plan;
// core/gitPane.js and core/changesReview.js hold the reviewer's dial.

import { patchHunks } from "./diff.js";

/** Ranked so a file takes the level of its most demanding hunk. */
const LEVEL_RANK = { low: 0, normal: 1, critical: 2 };

/** The group a low hunk falls into when the pass named none. */
const UNNAMED_GROUP = "Low risk";

/** The group a hunk the READER collapsed falls into when it has no group of its
 *  own — their judgment is not the pass's "low risk", and does not borrow its
 *  name. */
const READER_COLLAPSED_GROUP = "Collapsed by you";

/** Where the per-project trust-dial preference lives in localStorage. */
const TRUST_DIAL_KEY_PREFIX = "build.triage.dial.";

/** A level token as the plan speaks it, defaulting anything unknown to normal —
 *  an unrecognised level must not silently drop a hunk out of the stack. */
function levelOf(token) {
  return Object.hasOwn(LEVEL_RANK, token) ? token : "normal";
}

/** The pass's classification per hunk id, with the reviewer's overrides applied
 *  over it: `surface` puts a hunk back in the stack, `collapse` puts one in a
 *  group. The reviewer's direction wins — that is what an override is. */
function classifiedHunks(triage) {
  const byId = new Map();
  for (const hunk of (triage && triage.hunks) || []) {
    if (!hunk || !hunk.hunk_id) continue;
    byId.set(hunk.hunk_id, {
      level: levelOf(hunk.level),
      rationale: hunk.rationale || "",
      group: hunk.group || "",
      overridden: false,
      overrideDirection: "",
      note: "",
    });
  }
  for (const override of (triage && triage.overrides) || []) {
    if (!override || !override.hunk_id) continue;
    const classified = byId.get(override.hunk_id);
    if (!classified) continue;
    const said = { overridden: true, overrideDirection: override.direction, note: override.note || "" };
    if (override.direction === "surface") {
      byId.set(override.hunk_id, { ...classified, ...said, level: "normal", group: "" });
    } else if (override.direction === "collapse") {
      byId.set(override.hunk_id, {
        ...classified,
        ...said,
        level: "low",
        group: classified.group || READER_COLLAPSED_GROUP,
      });
    }
  }
  return byId;
}

/** The two ways a reviewer can disagree with a pass, as the wire says them. */
const OVERRIDE_DIRECTIONS = ["surface", "collapse"];

/**
 * The disagreement one hunk offers the reviewer, or null when it offers none.
 *
 * A surfaced critical offers to be collapsed and a collapsed hunk offers to be
 * kept surfaced — the two moves the doc names. A hunk the reviewer has already
 * moved offers the way back, so nothing is a one-way door. A hunk the pass left
 * in the middle of the stack, or never named at all, offers nothing: there is
 * no decision there to disagree with, and a control on every row would be noise
 * over the whole diff.
 */
export function overrideDirectionFor(mark) {
  if (!mark || !mark.hunk_id || mark.untriaged) return null;
  if (mark.overridden) return mark.overrideDirection === "surface" ? "collapse" : "surface";
  if (mark.level === "critical") return "collapse";
  if (mark.level === "low") return "surface";
  return null;
}

/**
 * applyTriageOverride(triage, { hunk_id, direction, note }) → a NEW triage
 * carrying that disagreement.
 *
 * This is how an override reaches the screen before the bridge has answered:
 * the same join that renders the pass renders the pass-plus-what-the-reviewer-
 * just-said, so the stack re-orders on the tap rather than on the next poll.
 * At most one word per hunk — saying it again replaces what was said before,
 * which is what the bridge records too.
 */
export function applyTriageOverride(triage, { hunk_id, direction, note = "" } = {}) {
  if (!triage) throw new Error("triage override: there is no pass to disagree with");
  if (!hunk_id) throw new Error("triage override: a disagreement names the hunk it is about");
  if (!OVERRIDE_DIRECTIONS.includes(direction))
    throw new Error(`triage override: unknown direction ${direction} — expected surface or collapse`);
  const kept = (triage.overrides || []).filter((override) => override.hunk_id !== hunk_id);
  return { ...triage, overrides: [...kept, { hunk_id, direction, note: note || "" }] };
}

export function applyTriageOverrides(triage, overrides) {
  return (overrides || []).reduce((carried, override) => applyTriageOverride(carried, override), triage);
}

/** The overrides a client has sent that the pass has not come back carrying —
 *  what it still has to hold on top of the bridge's answer. An override the
 *  pass now states is the bridge's to render, and holding a copy of it would
 *  only mean rendering the reviewer's decision from two places. */
export function unsettledOverrides(triage, pending) {
  const settled = new Map(((triage && triage.overrides) || []).map((override) => [override.hunk_id, override.direction]));
  return (pending || []).filter((override) => settled.get(override.hunk_id) !== override.direction);
}

/** The hunk ids of `patch`, queued per file path in patch order, so a file that
 *  appears twice in one patch still reads its own ids in sequence. */
function hunkIdQueues(patch) {
  const queues = new Map();
  for (const hunk of patchHunks(patch)) {
    if (!queues.has(hunk.path)) queues.set(hunk.path, []);
    queues.get(hunk.path).push(hunk.hunk_id);
  }
  return queues;
}

/** How many hunk rows a parsed file holds — what the ids are matched against. */
function hunkRowCount(file) {
  return ((file && file.rows) || []).filter((row) => row.t === "hunk").length;
}

/** One file's hunks, in order, as the render reads them: the level that applies,
 *  the rationale to show, and whether the pass said anything about this hunk at
 *  all. */
function markFile(file, queue, classified) {
  const marks = [];
  for (let index = 0; index < hunkRowCount(file); index++) {
    const hunkId = queue && index < queue.length ? queue[index] : null;
    const known = hunkId ? classified.get(hunkId) : undefined;
    marks.push({
      hunk_id: hunkId,
      level: known ? known.level : "normal",
      rationale: known ? known.rationale : "",
      untriaged: !known,
      overridden: Boolean(known && known.overridden),
      overrideDirection: known ? known.overrideDirection : "",
      note: known ? known.note : "",
      group: known ? known.group : "",
    });
  }
  return marks;
}

/** The one line a group header can show for a hunk: why it is folded away. On
 *  a hunk the READER folded that is whatever they said about it — the pass's
 *  own line argued for reading the hunk, and reusing it as the reason for not
 *  reading it would put words in nobody's mouth. */
function collapseReason(mark) {
  return mark.overridden ? mark.note : mark.rationale;
}

/** The section a marked file belongs to: any critical hunk surfaces the whole
 *  file (its low hunks come with it, in place — a file is read as a unit), a
 *  file that is nothing but low hunks collapses into the group its first low
 *  hunk names, and everything else is the normal stack. */
function sectionOf(marks) {
  if (marks.some((mark) => mark.level === "critical")) return { kind: "critical" };
  if (marks.length && marks.every((mark) => mark.level === "low")) {
    const named = marks.find((mark) => mark.group);
    return { kind: "group", name: named ? named.group : UNNAMED_GROUP };
  }
  return { kind: "normal" };
}

/** The plain stack: what a changeset with nothing to order by renders as. The
 *  files come through untouched — no marks, so no chips — and the surface says
 *  "untriaged" once, above the stack, instead of on every hunk. */
function plainPlan(files, status) {
  const list = files || [];
  return {
    status,
    sections: [{ kind: "normal", files: list, fileCount: list.length, hunkCount: list.reduce((total, file) => total + hunkRowCount(file), 0) }],
    counts: { critical: 0, normal: 0, low: 0, untriaged: 0 },
  };
}

/**
 * planChangesetTriage({ files, patch, triage }) → the render plan for ONE
 * changeset's stack.
 *
 * `files` is core/diff.js parseDiff output (noise already split off by the
 * caller — the generated-files group is its own thing, at the very bottom),
 * `patch` is the same patch those files came from (the hunk ids are derived
 * from it), and `triage` is the run's triage payload
 * (`{ based_on, hunks, overrides, stale }`) or null.
 *
 * The plan is `{ status, sections, counts }`. `status` is "ordered", "stale"
 * (the diff moved under the pass — order by it anyway and say so), or "none"
 * (no pass, or a pass that names nothing in this changeset). Sections come out
 * in render order: critical, normal, then one per named group.
 */
export function planChangesetTriage({ files = [], patch = "", triage = null } = {}) {
  const classified = classifiedHunks(triage);
  if (classified.size === 0) return plainPlan(files, "none");

  const queues = hunkIdQueues(patch);
  const taken = new Map(); // path → how many of its ids are already spoken for
  const critical = [];
  const normal = [];
  const groups = new Map(); // name → { files, hunkCount, rationale }
  const counts = { critical: 0, normal: 0, low: 0, untriaged: 0 };
  let matchedAny = false;

  for (const file of files) {
    const queue = (queues.get(file.path) || []).slice(taken.get(file.path) || 0);
    taken.set(file.path, (taken.get(file.path) || 0) + hunkRowCount(file));
    const marks = markFile(file, queue, classified);
    for (const mark of marks) {
      counts[mark.level] += 1;
      if (mark.untriaged) counts.untriaged += 1;
      else matchedAny = true;
    }
    const marked = { ...file, triageHunks: marks };
    const section = sectionOf(marks);
    if (section.kind === "critical") critical.push(marked);
    else if (section.kind === "normal") normal.push(marked);
    else {
      if (!groups.has(section.name)) groups.set(section.name, { files: [], hunkCount: 0, rationale: "" });
      const group = groups.get(section.name);
      group.files.push(marked);
      group.hunkCount += marks.length;
      if (!group.rationale) group.rationale = marks.map(collapseReason).find(Boolean) || "";
    }
  }

  // A pass about some other changeset orders nothing here: rather than paint
  // every hunk "untriaged", the stack renders plain and says so once.
  if (!matchedAny) return plainPlan(files, "none");

  const sections = [];
  if (critical.length)
    sections.push({ kind: "critical", files: critical, fileCount: critical.length, hunkCount: counts.critical });
  if (normal.length)
    sections.push({
      kind: "normal",
      files: normal,
      fileCount: normal.length,
      hunkCount: normal.reduce((total, file) => total + file.triageHunks.length, 0),
    });
  for (const [name, group] of groups)
    sections.push({
      kind: "group",
      name,
      rationale: group.rationale,
      files: group.files,
      fileCount: group.files.length,
      hunkCount: group.hunkCount,
    });
  return { status: triage && triage.stale ? "stale" : "ordered", sections, counts };
}

/** The one line the banner says about an ordered stack: what was surfaced, what
 *  was collapsed, and what the pass never saw. */
export function triageSummaryLine(plan) {
  const counts = (plan && plan.counts) || { critical: 0, low: 0, untriaged: 0 };
  const parts = [
    `${counts.critical} hunk${counts.critical === 1 ? "" : "s"} need${counts.critical === 1 ? "s" : ""} review first`,
    `${counts.low} collapsed`,
  ];
  if (counts.untriaged) parts.push(`${counts.untriaged} untriaged`);
  return parts.join(" · ");
}

/** The reviewer's trust dial for one project: on means "show me the untriaged
 *  full stack". Browser-scoped (localStorage), like every other preference this
 *  client keeps; a surface with no project to key it by keeps none. */
export function loadTrustDial(projectId, storage = globalThis.localStorage) {
  if (!projectId || !storage) return false;
  return storage.getItem(TRUST_DIAL_KEY_PREFIX + projectId) === "1";
}

export function saveTrustDial(projectId, on, storage = globalThis.localStorage) {
  if (!projectId || !storage) return;
  if (on) storage.setItem(TRUST_DIAL_KEY_PREFIX + projectId, "1");
  else storage.removeItem(TRUST_DIAL_KEY_PREFIX + projectId);
}

/** What a stack's ordering depends on, as one string: the revision the pass
 *  read, everything it said, everything the reviewer said back, and whether the
 *  diff has moved under it. A poll compares this to decide whether an otherwise
 *  untouched changeset has to be re-drawn — a re-pass over the same revision
 *  reclassifies hunks without moving a single byte of the diff, and the reader
 *  should see that the moment it lands. */
export function triageFingerprint(triage) {
  if (!triage) return "none";
  const hunks = (triage.hunks || [])
    .map((hunk) => `${hunk.hunk_id}:${hunk.level}:${hunk.group || ""}:${hunk.rationale || ""}`)
    .join(",");
  const overrides = (triage.overrides || [])
    .map((override) => `${override.hunk_id}:${override.direction}:${override.note || ""}`)
    .join(",");
  return [triage.based_on || "", triage.stale ? "stale" : "fresh", hunks, overrides].join("\x1f");
}
