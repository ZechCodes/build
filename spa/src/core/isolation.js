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

/** What to call an isolation the bridge named — a git worktree standing for a
 *  word this client cannot read, so a row is never labelled with nothing. */
export function isolationLabel(name) {
  return isolationNamedOrDefault(name).label;
}

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

/** The control itself, empty: the select `mountIsolation` fills from the bridge
 *  — rendering it pre-filled would show a choice nobody has confirmed — and the
 *  three lines the lock, the save and a refusal speak through. Both views paint
 *  it, so neither names the hooks the mount reaches for. */
export function isolationFieldHtml() {
  return `<div class="field"><label>Work isolation</label>
        <select data-isolation="select" style="width:100%" disabled><option>loading…</option></select></div>
      <div class="dim" data-isolation="lock" style="font-size:12px"></div>
      <div class="dim" data-isolation="saved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" data-isolation="error"></div>`;
}

/** The Settings panel: the control with the account's own heading and the
 *  sentence naming what each isolation gives you. */
export function isolationPanelHtml() {
  return `<div class="panel">
      <h3>🗂️ Work isolation</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">A copy-on-write clone starts with the project's build caches already in place and keeps its own git repository. A git worktree shares the project's repository and starts empty.</div>
      ${isolationFieldHtml()}
    </div>`;
}

/** The account's own choice: settings.set, and nothing above it to inherit. */
export const ACCOUNT_ISOLATION = { rpc: "settings.set", params: {}, inheritLabel: null };

/** A project's override, keyed on its id, naming the account default it
 *  replaces so the inherit option reads as what choosing it does. */
export function projectIsolationTarget(project) {
  return {
    rpc: "project.set_isolation",
    params: { project_id: project.project_id },
    inheritLabel: `Account default (${isolationLabel(project && project.isolation_default)})`,
  };
}

/** What the select shows: a project's own override, which may be nothing at
 *  all, and the account's answer defaulted because it has nothing above it. */
const chosenIsolation = (settings, inheritLabel) =>
  inheritLabel ? (settings && settings.isolation) || null : isolationOf(settings);

const lockLine = (available) => {
  const reason = isolationLockReason(available);
  return reason ? `Locked to git worktrees on this device: ${reason}.` : "";
};

/** Wire a select to one place a chosen isolation is sent — `ACCOUNT_ISOLATION`
 *  or `projectIsolationTarget(project)` — and to the payload that place answers
 *  with. The control paints from what the caller already read, saves the choice
 *  through the target, and repaints from the bridge's answer, so it shows what
 *  the bridge holds rather than what was merely asked for. A refusal is the
 *  bridge's own sentence, and the control goes back to the answer it last had. */
export async function mountIsolation(host, { callRpc, target, settings }) {
  const select = host.querySelector("[data-isolation=select]");
  if (!select) return;
  const lock = host.querySelector("[data-isolation=lock]");
  const saved = host.querySelector("[data-isolation=saved]");
  const error = host.querySelector("[data-isolation=error]");

  let held = settings;
  const paint = (state) => {
    held = state;
    const available = state && state.isolation_available;
    select.innerHTML = isolationOptionsHtml(chosenIsolation(state, target.inheritLabel), available, {
      inheritLabel: target.inheritLabel,
    });
    lock.textContent = lockLine(available);
  };

  paint(settings);
  select.disabled = false;

  select.onchange = async () => {
    const chosen = select.value || null;
    select.disabled = true;
    error.textContent = "";
    saved.textContent = "Saving…";
    try {
      paint(await callRpc(target.rpc, { ...target.params, isolation: chosen }));
      saved.textContent = "Saved. New worktrees are isolated this way; the ones already here keep what they were made with.";
    } catch (refusal) {
      error.textContent = refusal.message;
      saved.textContent = "";
      paint(held);
    }
    select.disabled = false;
  };
}
