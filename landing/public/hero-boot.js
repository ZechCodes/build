// Runs in the head, before the first paint: decides whether this visit
// plays the hero's entrance, so the first frame is already the notification
// field, or shows the hero at rest. Once per tab: entrance.js records the
// play in sessionStorage. A visitor who asked for less motion, or arrived
// on a link to a later act, gets the hero at rest. ?hero=play forces the
// entrance and ?hero=0 skips it, for checks.
(function () {
  var root = document.documentElement;
  var PLAYED_KEY = "build.hero.played";
  var requested = new URLSearchParams(location.search).get("hero");

  function playedThisTab() {
    try {
      return Boolean(sessionStorage.getItem(PLAYED_KEY));
    } catch (error) {
      // Storage refused: the entrance plays, as on a first visit.
      return false;
    }
  }

  if (requested === "0") return;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (requested !== "play") {
    if (/^#act-\d+$/.test(location.hash) && location.hash !== "#act-1") return;
    if (playedThisTab()) return;
  }
  root.dataset.hero = "entrance";
})();
