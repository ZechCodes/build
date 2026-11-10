// Settings manages metadata only. Add secret navigates to the existing
// composer; it never uploads a value or sends a message.
import { go } from "../app.js";
import { readCached, writeCached, subscribeCache } from "./localCache.js";
import { readEnvironmentSupport } from "./environmentSupport.js";
import { createSecretMetadataCache } from "./secretMetadataCache.js";
import { bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";
import { environmentAction } from "./environmentWrites.js";
import { navigateToEnvironmentSecret } from "./environmentSecretNavigation.js";
import { secretChipLabel, secretOverridesEnvironment } from "./secretLabels.js";
import { esc } from "./text.js";

const membersOf = (projects, id) => projects.filter(project => project.environment_id === id && !project.archived_at);
const auditFields = ["sequence", "secret_id", "creator", "project_id", "environment_id", "owner_id", "agent_id", "conversation_id", "generation", "session_incarnation", "request_id", "occurred_at", "attempt_count", "last_occurred_at", "outcome"];
function auditEntry(row) {
  const scalar = value => value === null || Number.isSafeInteger(value) || (typeof value === "string" && value.length <= 256);
  return Object.fromEntries(auditFields.filter(key => Object.hasOwn(row, key) && scalar(row[key])).map(key => [key, row[key]]));
}
const canRevoke = entry => ["staged", "available", "unavailable"].includes(entry.status);

function rowsHtml(entries, projects, projectEntries) {
  return entries.map(entry => {
    const overrides = projects.filter(project => (projectEntries.get(project.project_id) || [])
      .some(secret => secretOverridesEnvironment(secret, [entry])));
    return `<div class="ps-source"><p>${esc(secretChipLabel(entry))} · ${esc(entry.status)}</p>
      ${overrides.map(project => `<p class="sub">${esc(project.name)} <span class="badge secret-override-chip">overrides environment</span></p>`).join("")}
      <div class="row">${canRevoke(entry) ? `<button class="btn" data-secret-revoke="${esc(entry.secret_id)}">Revoke</button>` : ""}
      <button class="btn" data-secret-audit="${esc(entry.secret_id)}">Audit</button></div></div>`;
  }).join("");
}

function auditTimeHtml(occurredAt) {
  if (!Number.isSafeInteger(occurredAt)) return "";
  const date = new Date(occurredAt * 1000);
  return Number.isNaN(date.getTime()) ? "" : ` · ${esc(date.toLocaleString())}`;
}

function auditHtml(audit) {
  if (!audit) return "";
  const rows = audit.events.map(row => `<li>${esc(row.outcome)} · ${esc(row.agent_id || row.creator || "Build")}${auditTimeHtml(row.occurred_at)}</li>`).join("");
  return `<div data-secret-audit-results><h5>Audit</h5>${rows ? `<ol>${rows}</ol>` : '<p class="dim">No audit events yet.</p>'}
    ${audit.next === null ? "" : '<button class="btn" data-secret-audit-more>More audit events</button>'}</div>`;
}

export function createEnvironmentSettingsSecrets({ deviceId, environmentId, callRpc, navigate = go, current = () => true }) {
  let host = null, disposed = false, readGeneration = 0, supported = false, choosing = false;
  let entries = [], projects = [], projectEntries = new Map(), audit = null, auditId = null, error = "";
  const pending = new Set(), tracks = new Map();
  const active = () => !disposed && current();
  const scope = { environmentId };
  const cache = createSecretMetadataCache({ deviceId, call: callRpc,
    capabilities: () => bridgeCapabilities(deviceId).secrets, active });
  const auditAddress = id => ({ deviceId, entityId: id, kind: "secret-audit", sub: environmentId });

  function trackScopes() {
    const wanted = supported ? [scope, ...projects.map(project => ({ projectId: project.project_id, environmentId }))] : [];
    const keys = new Set(wanted.map(value => JSON.stringify(value)));
    for (const [key, stop] of tracks) if (!keys.has(key)) { stop(); tracks.delete(key); }
    for (const value of wanted) {
      const key = JSON.stringify(value);
      if (!tracks.has(key)) tracks.set(key, cache.track(value));
    }
  }
  async function read() {
    const mine = ++readGeneration;
    const [support, record, secrets] = await Promise.all([readEnvironmentSupport(deviceId),
      readCached({ deviceId, entityId: "", kind: "projects" }), cache.read(scope)]);
    const members = membersOf(record?.value || [], environmentId);
    const lists = await Promise.all(members.map(project => cache.read({ projectId: project.project_id })));
    const auditRecord = auditId ? await readCached(auditAddress(auditId)) : null;
    if (!active() || mine !== readGeneration) return;
    supported = support.secrets; projects = members; entries = secrets;
    projectEntries = new Map(members.map((project, index) => [project.project_id, lists[index]]));
    audit = auditRecord?.value || null;
    trackScopes(); paint();
  }
  function paint() {
    if (!active() || !host) return;
    if (!supported) { host.replaceChildren(); return; }
    host.innerHTML = `<section class="ps-section" data-environment-secrets><h4>Secrets</h4>
      <p class="sub">Available to current agents in every project in this environment.</p>
      ${rowsHtml(entries, projects, projectEntries) || '<p class="dim">No cached secrets.</p>'}
      <button class="btn" data-secret-add${projects.length ? "" : " disabled"}>Add secret</button>
      ${projects.length ? "" : '<p class="sub">An environment secret is added from a project in this environment.</p>'}
      ${choosing ? projects.map(project => `<button class="btn" data-secret-project="${esc(project.project_id)}">${esc(project.name)}</button>`).join("") : ""}
      ${auditHtml(audit)}<p class="adderr" role="alert">${esc(error)}</p></section>`;
    host.querySelector("[data-secret-add]").onclick = () => {
      if (projects.length === 1) openComposer(projects[0]);
      else { choosing = true; paint(); }
    };
    host.querySelectorAll("[data-secret-project]").forEach(button => {
      button.onclick = () => openComposer(projects.find(project => project.project_id === button.dataset.secretProject));
    });
    wireActions();
  }
  function openComposer(project) {
    if (project) navigateToEnvironmentSecret({ deviceId, environmentId, project, navigate });
  }
  async function action(key, send) {
    if (pending.has(key)) return;
    pending.add(key); error = ""; paint();
    try { await send(); }
    catch { error = "The secret action could not be confirmed. Try again."; }
    finally { pending.delete(key); if (active()) await read(); }
  }
  function wireActions() {
    host.querySelectorAll("[data-secret-revoke]").forEach(button => {
      const id = button.dataset.secretRevoke;
      button.disabled = pending.has(id);
      button.onclick = () => void action(id, async () => {
        const answer = await environmentAction(deviceId, "secrets", () => callRpc("secrets.revoke", {
          scope: { environment_id: environmentId }, secret_id: id, operation_id: crypto.randomUUID(),
        }));
        await cache.upsert(answer, scope);
      });
    });
    host.querySelectorAll("[data-secret-audit]").forEach(button => {
      button.disabled = pending.has("audit");
      button.onclick = () => void action("audit", () => loadAudit(button.dataset.secretAudit, 0));
    });
    const more = host.querySelector("[data-secret-audit-more]");
    if (more) { more.disabled = pending.has("audit"); more.onclick = () => void action("audit", () => loadAudit(auditId, audit.next)); }
  }
  async function loadAudit(id, after) {
    const answer = await environmentAction(deviceId, "secrets", () => callRpc("secrets.audit", {
      scope: { environment_id: environmentId }, secret_id: id, after, limit: 100,
    }));
    if (!active()) return;
    const events = answer.events.filter(row => row.secret_id === id).map(auditEntry);
    await writeCached(auditAddress(id), { events, next: answer.next ?? null });
    auditId = id;
  }
  const unsubscribe = subscribeCache({ deviceId }, address => {
    if (["secret-metadata", "secret-audit", "projects", "environment-support"].includes(address.kind)) void read();
  });
  const stopGreeting = onBridgeGreeted(id => { if (id === deviceId) void cache.refreshTracked().catch(() => {}); });
  void read();
  return Object.freeze({
    mount(root) { host = root; paint(); },
    dispose() { disposed = true; readGeneration += 1; unsubscribe(); stopGreeting(); tracks.forEach(stop => stop()); cache.dispose(); },
  });
}
