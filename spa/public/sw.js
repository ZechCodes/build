// Build push worker — deliberately tiny and cache-free.
//
// E2EE invariant: push payloads are content-free ({kind:"attention",url:"/app/"}),
// so every notification renders the same generic line. Task content only ever
// decrypts inside the open app. There is NO fetch handler on purpose: an E2EE
// app must never be served from a stale cache.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let url = "/app/";
  try {
    const payload = event.data ? event.data.json() : null;
    if (payload && typeof payload.url === "string" && payload.url.startsWith("/")) {
      url = payload.url;
    }
  } catch {
    // Not JSON — render the generic notification anyway.
  }
  event.waitUntil(
    self.registration.showNotification("Build", {
      body: "A task needs your attention.",
      tag: "build-attention",
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
      if (existing) return existing.focus();
      return self.clients.openWindow(url);
    })(),
  );
});
