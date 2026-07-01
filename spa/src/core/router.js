// Hash routes: #/board, #/notifications, #/settings, #/task/<id>/<tab>.
// Pure mapping both ways; the app shell owns the hashchange listener.

export function routeFromHash(hash) {
  const parts = (hash || "").replace(/^#\/?/, "").split("/").filter(Boolean);
  switch (parts[0]) {
    case "notifications":
      return { name: "notifications" };
    case "settings":
      return { name: "settings" };
    case "task":
      if (!parts[1]) return { name: "board" };
      return {
        name: "task",
        id: decodeURIComponent(parts[1]),
        tab: parts[2] === "diff" ? "diff" : "plan",
      };
    default:
      return { name: "board" };
  }
}

export function hashFromRoute(route) {
  if (route.name === "task") return `#/task/${encodeURIComponent(route.id)}/${route.tab || "plan"}`;
  if (route.name === "notifications") return "#/notifications";
  if (route.name === "settings") return "#/settings";
  return "#/board";
}
