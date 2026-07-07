// Build push worker — deliberately tiny and cache-free.
//
// E2EE invariant: push payloads carry only opaque metadata ({task_id, kind, url}),
// never task content (goals/plan text). The service worker renders kind-specific
// copy from `kind`; the actual task state only decrypts inside the open app.
// There is NO fetch handler on purpose: an E2EE app must never be served from a
// stale cache.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

// Kind → the generic line shown on the notification. Unknown kinds fall back to
// the attention copy, so a new bridge kind never renders blank.
const KIND_BODY = {
  plan_ready: "Plan ready to review",
  task_done: "Task finished — diff ready",
  blocked: "Agent needs your attention",
  attention: "Agent needs your attention",
};

function bodyForKind(kind) {
  return KIND_BODY[kind] || KIND_BODY.attention;
}

self.addEventListener("push", (event) => {
  let url = "/app/";
  let kind = "attention";
  let taskId = "";
  try {
    const payload = event.data ? event.data.json() : null;
    if (payload) {
      if (typeof payload.url === "string" && payload.url.startsWith("/")) url = payload.url;
      if (typeof payload.kind === "string") kind = payload.kind;
      if (typeof payload.task_id === "string") taskId = payload.task_id;
    }
  } catch {
    // Not JSON — render the generic notification anyway.
  }
  // tag=task_id so repeated pushes for the same task collapse into one
  // notification instead of stacking; a payload without a task id falls back to a
  // single shared tag.
  const tag = taskId ? `build-task-${taskId}` : "build-attention";
  event.waitUntil(
    self.registration.showNotification("Build", {
      body: bodyForKind(kind),
      tag,
      renotify: true,
      data: { url },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || "/app/";
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });
      const existing = windows.find((w) => new URL(w.url).pathname.startsWith("/app"));
      if (existing) {
        // Focus the open app and steer it to the deep link so the click always
        // lands on the right task, not just whatever was last open.
        await existing.focus();
        if ("navigate" in existing) {
          try {
            await existing.navigate(url);
          } catch {
            // Cross-origin or unsupported — the focus above is enough.
          }
        }
        return existing;
      }
      return self.clients.openWindow(url);
    })(),
  );
});
