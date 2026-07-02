// Model/effort selector logic. The catalog comes from the bridge's models.list
// RPC — the UI never hardcodes model ids, so new models arrive with bridge
// updates. An empty value means "harness default": the user's own Claude Code
// configuration decides, and the dispatch params omit the field entirely.

import { esc } from "./text.js";

/** <option> list for the model select: harness default, catalog, and — when the
 *  current selection is not in the catalog (e.g. a task dispatched on a newer
 *  bridge) — the selection itself, so it never silently changes. */
export function modelOptionsHtml(models, selectedId) {
  const sel = (id) => (id === (selectedId || "") ? " selected" : "");
  const rows = [`<option value=""${sel("")}>Harness default</option>`];
  for (const m of models) {
    rows.push(`<option value="${esc(m.id)}"${sel(m.id)}>${esc(m.label)}</option>`);
  }
  if (selectedId && !models.some((m) => m.id === selectedId)) {
    rows.push(`<option value="${esc(selectedId)}" selected>${esc(selectedId)}</option>`);
  }
  return rows.join("");
}

/** Whether the effort select applies to the chosen model. Unknown ids pass
 *  through (the bridge validates); only catalog models that declare no effort
 *  support disable it. */
export function effortSupported(models, selectedId) {
  if (!selectedId) return true;
  const entry = models.find((m) => m.id === selectedId);
  return entry ? entry.supports_effort : true;
}

export function effortOptionsHtml(efforts, selected) {
  const sel = (v) => (v === (selected || "") ? " selected" : "");
  return [
    `<option value=""${sel("")}>Default effort</option>`,
    ...efforts.map((e) => `<option value="${esc(e)}"${sel(e)}>${esc(e)}</option>`),
  ].join("");
}

/** The dispatch/approve params fragment for a selection: empties are omitted
 *  (harness default), and effort is dropped for models that don't support it. */
export function modelParams(models, model, effort) {
  const params = {};
  if (model) params.model = model;
  if (effort && effortSupported(models, model)) params.effort = effort;
  return params;
}
