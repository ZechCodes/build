// Runs in the head, before the first paint: decides whether this visit
// plays the hero's entrance, so the first frame is already the notification
// wall, or shows the hero at rest. A visitor who asked for less motion, or arrived
// on a link to a later act, gets the hero at rest. ?hero=play forces the
// entrance and ?hero=0 skips it, for checks.
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
