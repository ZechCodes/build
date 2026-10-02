// Unlisted #336 preview: replay on every load, ignoring per-tab memory.
(function () {
  var root = document.documentElement;
  var requested = new URLSearchParams(location.search).get("hero");

  if (requested === "0") return;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (requested !== "play") {
    if (/^#act-\d+$/.test(location.hash) && location.hash !== "#act-1") return;
  }
  root.dataset.hero = "entrance";
})();
