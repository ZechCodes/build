// The page's entry point. Bundled by Astro into /landing/generated/_astro/ as
// an external module, because the app's CSP (script-src 'self'
// 'wasm-unsafe-eval') drops inline script.
//
// The document is complete and readable without it. This starts the film,
// the pinned, scroll-driven version with the three.js device stage, for the
// visitors film-boot.js chose. Phones, reduced motion, save-data and
// machines without WebGL keep the document.

// The document's cue to stop: an act's copy comes in as the act reaches the
// upper part of the window, once (landing.css). Acts already there are
// marked before the copy is hidden, so nothing on screen blinks.
function watchActArrivals() {
  const acts = [...document.querySelectorAll("[data-act]")];
  const arrive = (act) => { act.dataset.arrived = ""; };
  if (typeof IntersectionObserver !== "function") {
    acts.forEach(arrive);
    return;
  }
  const line = innerHeight * 0.65;
  const waiting = acts.filter((act) => {
    if (act.getBoundingClientRect().top >= line) return true;
    arrive(act);
    return false;
  });
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      arrive(entry.target);
      observer.unobserve(entry.target);
    }
  }, { rootMargin: "0px 0px -35% 0px" });
  waiting.forEach((act) => observer.observe(act));
}

watchActArrivals();
document.documentElement.dataset.js = "ready";

// film-boot.js, in the head, already chose before the first paint and set
// data-mode="film"; this only starts what it chose. ?film=force skips the
// frame budget, for automated checks on software renderers. If the boot's
// deadline gave the page back to the document first, the film stays out.
const root = document.documentElement;
const params = new URLSearchParams(location.search);
const requested = params.get("film");

function backToDocument(error) {
  console.warn("The film could not start; the document stands.", error);
  delete root.dataset.mode;
  delete root.dataset.stage;
  // A call to action pressed while the film was pending still goes where it
  // pointed, now in the document.
  if (/^#act-\d+$/.test(location.hash)) document.getElementById(location.hash.slice(1))?.scrollIntoView();
}

if (root.dataset.mode === "film") {
  import("../film/film.js")
    .then(({ startFilm }) => {
      if (root.dataset.mode !== "film") return;
      root.dataset.stage = "starting";
      startFilm({ ignoreFrameBudget: requested === "force" });
    })
    .catch(backToDocument);
}
