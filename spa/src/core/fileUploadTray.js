import "../styles/fileUploads.css";

import { humanBytes as bytes } from "./workspaceLifecycle.js";

function element(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(text, label, action) {
  const node = element("button", "fupload-action", text);
  node.type = "button";
  node.setAttribute("aria-label", label);
  node.addEventListener("click", action);
  return node;
}

function progress(received, size) {
  const node = element("progress", "fupload-progress");
  node.max = Math.max(1, size);
  node.value = received;
  node.setAttribute("aria-label", "Upload progress");
  return node;
}

function uploadItem(item, uploads, recent, support) {
  const row = element("li", "fupload-item");
  row.append(element("div", "fupload-name", item.name));
  row.append(element("div", "fupload-destination", item.destination || item.parent || "/"));
  const detail = element("div", "fupload-detail");
  if (recent) {
    const time = new Date(item.finishedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    detail.textContent = `${item.status} · ${time}`;
  } else {
    detail.textContent = `${bytes(item.received)} / ${bytes(item.size)}`;
    row.append(progress(item.received, item.size));
  }
  row.append(detail);
  if (item.error) row.append(element("div", "fupload-error", item.error));
  appendActions(row, item, uploads, recent, support);
  return row;
}

function appendActions(row, item, uploads, recent, support) {
  const actions = element("div", "fupload-actions");
  if (!recent) {
    const cancel = button(item.status === "cancelling" ? "Cancelling…" : "Cancel", `Cancel ${item.name}`, () => uploads.cancel(item.id));
    cancel.disabled = item.status === "cancelling";
    actions.append(cancel);
  } else if (item.status === "failed" && item.canRetry && support.uploads) {
    actions.append(button("Retry", `Retry ${item.name}`, () => uploads.retry(item.id, { replace: false })));
    if (item.errorCode === "already_exists") actions.append(button("Replace", `Replace ${item.name}`, () => uploads.retry(item.id, { replace: true })));
  }
  row.append(actions);
}

function summaryOf(active, recent) {
  if (!active.length) return `${recent.length} ${recent.length === 1 ? "upload" : "uploads"} finished`;
  const size = active.reduce((sum, item) => sum + item.size, 0);
  const received = active.reduce((sum, item) => sum + item.received, 0);
  const percent = size ? Math.min(100, Math.round(received / size * 100)) : 0;
  return `Uploading ${active.length} ${active.length === 1 ? "file" : "files"} · ${percent}%`;
}

/** Paints the engine's in-memory upload state, confined to the file viewer. */
export function mountFileUploadTray(viewerEl, { uploads, capabilities = { uploads: true } }) {
  let support = capabilities;
  let expanded = false;
  let showRecent = false;
  let disposed = false;
  const tray = element("section", "fupload-tray");
  tray.setAttribute("aria-label", "File uploads");
  const summary = button("", "Show uploads", () => { expanded = !expanded; render(); });
  summary.className = "fupload-summary";
  const status = element("span", "fupload-status");
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");
  summary.replaceChildren(status);
  const aggregate = progress(0, 1);
  const body = element("div", "fupload-body");
  tray.append(summary, aggregate, body);
  viewerEl.append(tray);

  function render() {
    if (disposed) return;
    const snapshot = uploads.snapshot();
    const { active, recent } = snapshot;
    tray.hidden = !active.length && !recent.length;
    status.textContent = summaryOf(active, recent);
    summary.setAttribute("aria-expanded", String(expanded));
    summary.setAttribute("aria-label", expanded ? "Hide uploads" : "Show uploads");
    aggregate.hidden = !active.length;
    aggregate.max = Math.max(1, active.reduce((sum, item) => sum + item.size, 0));
    aggregate.value = active.reduce((sum, item) => sum + item.received, 0);
    body.hidden = !expanded;
    const focusLabel = body.contains(document.activeElement) ? document.activeElement.getAttribute("aria-label") : null;
    renderBody(active, recent);
    if (focusLabel) restoreFocus(focusLabel);
  }

  function restoreFocus(label) {
    for (const control of body.querySelectorAll("button")) {
      if (control.getAttribute("aria-label") === label) { control.focus({ preventScroll: true }); break; }
    }
  }

  function renderBody(active, recent) {
    body.replaceChildren();
    const list = element("ul", "fupload-list");
    for (const item of active) list.append(uploadItem(item, uploads, false, support));
    body.append(list);
    if (!recent.length) return;
    const toggle = button(`Recent (${recent.length})`, "Recent uploads", () => { showRecent = !showRecent; render(); });
    toggle.className = "fupload-recent-toggle";
    toggle.setAttribute("aria-expanded", String(showRecent));
    body.append(toggle);
    if (!showRecent) return;
    const history = element("ul", "fupload-list");
    for (const item of recent) history.append(uploadItem(item, uploads, true, support));
    body.append(history);
  }

  uploads.prune();
  const unsubscribe = uploads.subscribe(render);
  const timer = setInterval(() => { uploads.prune(); render(); }, 60_000);
  render();
  const dispose = () => { disposed = true; clearInterval(timer); unsubscribe(); tray.remove(); };
  dispose.setCapabilities = (next) => { support = next; render(); };
  return dispose;
}
