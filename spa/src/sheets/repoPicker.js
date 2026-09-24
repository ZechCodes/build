// A Git remote URL input that searches the machine's GitHub repositories.
//
// `attachRepoPicker` turns an input a sheet already painted into a combobox
// over the device's cached github.repos list (core/githubRepos.js): typing
// filters it, arrows and Enter or a click pick a row, and picking writes the
// row's SSH URL into the input and fires the input's own `input` event, so the
// sheet reacts to a pick exactly as it does to typing. Enter with no row
// chosen is left to the form, so a typed URL goes through untouched.
//
// It paints only from the cache. With no list cached it adds nothing to the
// field; with no list and a refusal cached it says the bridge's sentence under
// the first remote input of the sheet, once.

import "../styles/repoPicker.css";
import { esc } from "../core/text.js";
import { ICON_LOCK } from "../core/icons.js";
import { heldGithubRepos, rankRepos, watchGithubRepos } from "../core/githubRepos.js";

let serial = 0;

const optionHtml = (listId) => (repo, index) => `<li role="option" id="${listId}-${index}" class="repo-picker-option" aria-selected="false">
  <span class="repo-picker-title"><span class="repo-picker-name">${esc(repo.name_with_owner)}</span>${repo.private ? `<span class="repo-picker-lock" role="img" aria-label="Private">${ICON_LOCK}</span>` : ""}</span>
  ${repo.description ? `<span class="repo-picker-desc">${esc(repo.description)}</span>` : ""}</li>`;

/** The repositories to search, or null when the field stays a plain one. */
const searchable = (record) => (Array.isArray(record?.repos) && record.repos.length ? record.repos : null);

/** Whether another picker in the same sheet already says the refusal. */
const saidElsewhere = (note) => {
  const scope = note.closest("#sheet") || note.ownerDocument;
  return [...scope.querySelectorAll(".repo-picker-note:not([hidden])")].some((other) => other !== note);
};

/** Make `input` search `deviceId`'s repositories. Does nothing without a
 *  device, and nothing to an input it already searches from. */
export function attachRepoPicker(input, deviceId) {
  if (!input || !deviceId || input.dataset.repoPicker) return;
  input.dataset.repoPicker = "on";
  const listId = `repo-picker-${++serial}`;
  const list = Object.assign(input.ownerDocument.createElement("ul"), { id: listId, className: "repo-picker-list", hidden: true });
  list.setAttribute("role", "listbox");
  list.setAttribute("aria-label", "GitHub repositories");
  const note = Object.assign(input.ownerDocument.createElement("div"), { className: "repo-picker-note", hidden: true });
  input.after(list, note);
  const state = { open: false, active: -1, matches: [], picking: false };

  const plain = () => {
    for (const name of ["role", "aria-autocomplete", "aria-expanded", "aria-controls", "aria-activedescendant"]) input.removeAttribute(name);
    list.hidden = true;
  };
  const combobox = () => {
    input.setAttribute("role", "combobox");
    input.setAttribute("aria-autocomplete", "list");
    input.setAttribute("aria-controls", listId);
  };
  const paintNote = (record) => {
    const sentence = !searchable(record) && record?.refusal ? record.refusal : "";
    note.textContent = sentence;
    note.hidden = !sentence || saidElsewhere(note);
  };
  const paintList = (repos) => {
    state.matches = state.open ? rankRepos(input.value, repos) : [];
    if (state.active >= state.matches.length) state.active = -1;
    list.innerHTML = state.matches.map(optionHtml(listId)).join("");
    list.hidden = !state.matches.length;
    input.setAttribute("aria-expanded", String(!list.hidden));
    const active = list.children[state.active];
    active?.setAttribute("aria-selected", "true");
    if (active) input.setAttribute("aria-activedescendant", active.id);
    else input.removeAttribute("aria-activedescendant");
    active?.scrollIntoView?.({ block: "nearest" });
  };
  const paint = () => {
    const record = heldGithubRepos(deviceId);
    paintNote(record);
    const repos = searchable(record);
    if (!repos) return plain();
    combobox();
    paintList(repos);
  };

  const openAt = (active) => { state.open = true; state.active = active; paint(); };
  const close = () => { state.open = false; state.active = -1; paint(); };
  const pick = (repo) => {
    input.value = repo.ssh_url;
    close();
    state.picking = true;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    state.picking = false;
  };
  const move = (step) => {
    if (!searchable(heldGithubRepos(deviceId))) return false;
    if (!state.open) openAt(-1);
    const count = state.matches.length;
    if (!count) return false;
    const next = state.active < 0 ? (step > 0 ? 0 : count - 1) : (state.active + step + count) % count;
    openAt(next);
    return true;
  };
  const choose = () => {
    const repo = state.open ? state.matches[state.active] : null;
    if (repo) pick(repo);
    return Boolean(repo);
  };
  const dismiss = () => {
    if (list.hidden) return false;
    close();
    return true;
  };
  const KEYS = { ArrowDown: () => move(1), ArrowUp: () => move(-1), Enter: choose, Escape: dismiss };

  input.addEventListener("input", () => { if (!state.picking) openAt(-1); });
  input.addEventListener("focus", () => openAt(-1));
  input.addEventListener("blur", close);
  input.addEventListener("keydown", (event) => {
    if (!KEYS[event.key]?.()) return;
    event.preventDefault();
    event.stopPropagation();
  });
  // Pressing a row must not blur the input first: that would close the list
  // under the click.
  list.addEventListener("mousedown", (event) => event.preventDefault());
  list.addEventListener("click", (event) => {
    const row = event.target.closest('[role="option"]');
    const repo = row ? state.matches[[...list.children].indexOf(row)] : null;
    if (repo) pick(repo);
  });

  const stop = watchGithubRepos(deviceId, () => {
    if (input.isConnected) paint();
    else stop();
  });
  paint();
}
