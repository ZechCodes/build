// Build push worker — deliberately tiny and cache-free.
//
// E2EE invariant: push payloads carry only opaque metadata ({task_id, kind, url}),
// never content. A push fires only for what adds to the unread counter (#191):
// an agent's conversation (`agent`) or a watched task (`task`). The service
// worker renders the kind's copy; the real state only decrypts inside the open
// app.
// There is NO fetch handler on purpose: an E2EE app must never be served from a
// stale cache.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

// Kind → the generic line shown on the notification. Unknown kinds fall back to
// the agent copy, so a new bridge kind never renders blank.
const KIND_BODY = {
  agent: "An agent needs you",
  task: "New activity on a task",
};

const NOTIFICATION_ICON = "/app/static/icon-192.png";

function bodyForKind(kind) {
  return KIND_BODY[kind] || KIND_BODY.agent;
}

self.addEventListener("push", (event) => {
  let url = "/app/";
  let kind = "agent";
  let taskId = "";
  try {
    const payload = event.data ? event.data.json() : null;
    if (payload) {
      // Only a same-origin absolute path is a safe deep link. Reject a
      // protocol-relative "//host/path" (a buggy payload) — it is cross-origin and
      // would navigate away from the app; fall back to the app root.
      if (typeof payload.url === "string" && payload.url.startsWith("/") && !payload.url.startsWith("//"))
        url = payload.url;
      if (typeof payload.kind === "string") kind = payload.kind;
      if (typeof payload.task_id === "string") taskId = payload.task_id;
    }
  } catch {
    // Not JSON — render the generic notification anyway.
  }
  // tag=task_id so repeated pushes for the same entity collapse into one
  // notification instead of stacking; a payload without an id falls back to a
  // single shared tag.
  const tag = taskId ? `build-task-${taskId}` : "build-attention";
  event.waitUntil(
    self.registration.showNotification("Build", {
      body: bodyForKind(kind),
      icon: NOTIFICATION_ICON,
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
          } catch (err) {
            // navigate() rejects for an uncontrolled window (shift-reload, mid-update)
            // or a malformed deep link. Don't swallow it silently: log it and open a
            // fresh window on the target so the click still lands there.
            console.warn("sw: deep-link navigate failed; opening a new window", err);
            return self.clients.openWindow(url);
          }
        }
        return existing;
      }
      return self.clients.openWindow(url);
    })(),
  );
});
