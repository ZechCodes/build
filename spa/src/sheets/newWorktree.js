// Naming a worktree, and saying which agent works in it.
//
// Both answers are needed before anything is created: the name decides the
// directory and the branch (the daemon slugifies it), and the agent decides
// which harness Build starts when the first turn is delivered there. So this
// asks for both up front and creates once — rather than minting an unnamed
// worktree and asking afterwards, which left a `build/worktree` nobody chose
// lying around whenever the chooser was dismissed.
//
// The choice is a PROVIDER, not a tab kind: a worktree has one agent, Build
// owns it, and it lives in the worktree's Agent tab. Nothing spawns here — the
// answer rides along as the run's provider and the surface opens on the tab
// that agent will appear in.

import { $ } from "../dom.js";
import { App, go, loadModelCatalog } from "../app.js";
import { normalizeModelCatalog, providerCardsHtml } from "../core/modelPicker.js";
import { markNewWorktree } from "../core/newWorktree.js";
import { notifyError } from "../core/notify.js";

export function openNewWorktree({ projectId } = {}) {
  let provider = null;
  $("#sheet").innerHTML = `
    <h3>New worktree</h3>
    <div class="sub">A branch and a directory of its own, with one agent of your choosing. Nothing is filed and no agent is dispatched — it is yours to drive.</div>
    <div class="field"><label>Name</label>
      <input id="wtname" class="path" placeholder="e.g. mascot model spike" style="width:100%" autocomplete="off" /></div>
    <div class="field" id="wtagent"></div>
    <div class="adderr" id="wterr"></div>
    <div class="row"><span class="dim mono" id="wtbranch" style="font-size:11px"></span>
      <button class="btn" id="wtcancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="wtcreate">Create</button></div>`;
  $("#scrim").classList.add("show");
  const name = $("#wtname");
  const create = $("#wtcreate");
  name.focus();

  // Create stays inert until BOTH answers are in — the button is the contract,
  // not a place to discover what is missing.
  const sync = () => {
    create.disabled = !name.value.trim() || !provider;
    // Echo the branch the daemon will cut, using its own slug rules, so the name
    // is not a surprise after the fact.
    $("#wtbranch").textContent = name.value.trim() ? `build/${slugPreview(name.value)}` : "";
  };
  name.oninput = () => {
    $("#wterr").textContent = "";
    sync();
  };

  // The providers come from the daemon's catalog, so a bridge that learns a new
  // harness offers it here without a client release. Until it answers, the
  // field stays empty rather than guessing a list.
  loadModelCatalog().then((catalog) => {
    const providers = normalizeModelCatalog(catalog).providers || [];
    const host = $("#wtagent");
    if (!host) return; // the sheet was dismissed while the catalog was in flight
    host.innerHTML = `<label>Agent</label>${providerCardsHtml(providers, provider)}`;
    host.querySelectorAll(".chooser-card").forEach((card) => {
      card.onclick = () => {
        provider = card.dataset.provider;
        host.querySelectorAll(".chooser-card").forEach((c) => c.classList.toggle("chosen", c === card));
        sync();
      };
    });
    sync();
  });
  sync();

  $("#wtcancel").onclick = () => $("#scrim").classList.remove("show");
  create.onclick = async () => {
    const wanted = name.value.trim();
    if (!wanted || !provider) return;
    create.disabled = true;
    create.textContent = "creating…";
    try {
      const created = await App.call("worktree.create", { project_id: projectId, name: wanted });
      // The surface opens on the Agent tab and dispatches with this provider.
      markNewWorktree(created.worktree_id, provider);
      $("#scrim").classList.remove("show");
      go({ name: "worktree", projectId: created.project_id || projectId, worktreeId: created.worktree_id });
    } catch (e) {
      create.disabled = false;
      create.textContent = "Create";
      $("#wterr").textContent = e.message;
      notifyError("Couldn't create the worktree", e.message);
    }
  };
}

/** The daemon's slug rules, mirrored for the preview only: ASCII alphanumerics,
 *  single hyphens, nothing else. The daemon is still the one that decides. */
export function slugPreview(raw) {
  return String(raw)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 50)
    .replace(/-+$/g, "");
}
