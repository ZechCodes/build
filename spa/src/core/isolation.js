// How a task's checkout is isolated from the project — the account's choice,
// overridable per project.
//
// A git worktree shares the project's repository, so it costs nothing and
// starts empty. A copy-on-write clone copies the whole project directory,
// `.git` included, through the filesystem, so it also costs nothing and starts
// warm — node_modules, target and every other ignored directory already there —
// but it is a repository of its own, and only a volume that can clone can make
// one. Which volumes those are is the bridge's fact, and so is the sentence
// naming why this one cannot: the client renders it, never writes it.
//
// This is the client's one naming table for the two isolations. The settings
// panel and the project sheet both read it, so neither learns a variant name, a
// label, an RPC shape or a locked look.

import { esc } from "./text.js";

/** The two isolations, in the order every control offers them. */
export const ISOLATIONS = [
  { id: "worktree", label: "Git worktree" },
  { id: "cow", label: "Copy-on-write clone" },
];

const isolationNamed = (name) => ISOLATIONS.find((isolation) => isolation.id === name);

/** A word from the bridge as one of the two, a git worktree standing for
 *  anything this client cannot name — the one rule for reading the wire. */
const isolationNamedOrDefault = (name) => isolationNamed(name) || ISOLATIONS[0];

/** The isolation a settings payload or a project row names, defaulted rather
 *  than left empty when it names none the client knows. */
export function isolationOf(settings) {
  return isolationNamedOrDefault(settings && settings.isolation).id;
}

/** Why this device cannot clone, or "" while it can. The bridge writes the
 *  sentence; an availability that refuses without one still says something,
 *  because a disabled control with no reason is a dead end. */
export function isolationLockReason(available) {
  if (!available || available.cow !== false) return "";
  return available.reason || "unavailable on this device";
}

/** The isolations as `<option>`s: the chosen one marked, the clone disabled and
 *  carrying the lock reason when the volume locks it, and — where a project
 *  inherits — the account's answer offered first as the empty value. */
export function isolationOptionsHtml(selected, available, { inheritLabel } = {}) {
  const lockReason = isolationLockReason(available);
  const chosen = isolationNamed(selected);
  const inherit = inheritLabel
    ? `<option value=""${chosen ? "" : " selected"}>${esc(inheritLabel)}</option>`
    : "";
  const options = ISOLATIONS.map((isolation) => {
    const locked = isolation.id === "cow" && lockReason;
    return `<option value="${esc(isolation.id)}"${isolation === chosen ? " selected" : ""}${
      locked ? ` disabled title="${esc(lockReason)}"` : ""
    }>${esc(isolation.label)}</option>`;
  }).join("");
  return `${inherit}${options}`;
}

/** The Settings panel, empty. `mountIsolation` fills the select from the bridge
 *  — rendering it pre-filled would show a choice nobody has confirmed is the
 *  account's — and speaks the lock, the save and a refusal through the three
 *  lines below it. */
export function isolationPanelHtml() {
  return `<div class="panel">
      <h3>🗂️ Work isolation</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">A copy-on-write clone starts with the project's build caches already in place and keeps its own git repository. A git worktree shares the project's repository and starts empty.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:180px"><label for="isolation">Work isolation</label>
          <select id="isolation" data-isolation="select" disabled><option>loading…</option></select></div>
      </div>
      <div class="dim" data-isolation="lock" style="font-size:12px"></div>
      <div class="dim" data-isolation="saved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" data-isolation="error"></div>
    </div>`;
}

/** The account's own choice: settings.set, and nothing above it to inherit. */
export const ACCOUNT_ISOLATION = { rpc: "settings.set", params: {}, inheritLabel: null };

/** A project's override, keyed on its id, naming the account default it
 *  replaces so the inherit option reads as what choosing it does. */
export function projectIsolationTarget(project) {
  const inherited = isolationNamedOrDefault(project && project.isolation_default);
  return {
    rpc: "project.set_isolation",
    params: { project_id: project.project_id },
    inheritLabel: `Account default (${inherited.label})`,
  };
}
