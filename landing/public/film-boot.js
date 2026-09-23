// Runs in the head, before the first paint, so a desktop visitor's first
// frame is already the film's layout: the hero copy and its call to action
// over the laptop, not the document's grid. The film module arrives later
// and takes over; if it never does (blocked, failed, no WebGL after all), the
// page goes back to the document. The rules are fallback.js's, repeated here
// because a module cannot run this early; test/stage/boot.test.mjs holds the
// two together.
(function () {
  var root = document.documentElement;
  var DOCUMENT_MAX_WIDTH = 768;
  var START_DEADLINE_MS = 8000;

  function hasWebgl() {
    try {
      return Boolean(document.createElement("canvas").getContext("webgl2"));
    } catch (error) {
      return false;
    }
  }

  var requested = new URLSearchParams(location.search).get("film");
  if (requested === "0") return;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (!(innerWidth >= DOCUMENT_MAX_WIDTH)) return;
  if (navigator.connection && navigator.connection.saveData) return;
  if (!hasWebgl()) return;

  root.dataset.mode = "film";
  root.dataset.stage = "pending";
  setTimeout(function () {
    // "starting": the module is in and owns its own fallback from here.
    if (root.dataset.stage === "starting" || root.dataset.stage === "ready") return;
    delete root.dataset.mode;
    delete root.dataset.stage;
    // A call to action pressed while the film was pending still goes where
    // it pointed, now in the document.
    var target = /^#act-\d+$/.test(location.hash) ? document.getElementById(location.hash.slice(1)) : null;
    if (target) target.scrollIntoView();
  }, START_DEADLINE_MS);
})();
