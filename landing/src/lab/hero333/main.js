// The page's entry point. Bundled by Astro into /landing/generated/_astro/ as
// an external module, because the app's CSP (script-src 'self'
// 'wasm-unsafe-eval') drops inline script.
//
// The document is complete and readable without it. This starts the film,
// the pinned, scroll-driven version with the three.js device stage, for the
// visitors film-boot.js chose. Phones, reduced motion, save-data and
// machines without WebGL keep the document.

import { restHero, startHeroEntrance } from "./entrance.js";

// The hero's entrance first, from wherever its field's CSS drift has got
// to. If it cannot play, the hero simply rests.
let hero = null;
try {
  hero = startHeroEntrance();
} catch (error) {
  console.warn("The hero entrance could not play; the hero stands.", error);
  restHero();
}
// For checks, as window.BuildFilm is: the entrance's timeline and state.
window.BuildHero = hero;
// Replay and a phase scrubber, for tuning only: under `astro dev`, or in a
// build made with PUBLIC_HERO_SCRUBBER=1 for the preview server (the
// entrance's assets are served there). A production build drops this
// branch and the module with it.
if ((import.meta.env.DEV || import.meta.env.PUBLIC_HERO_SCRUBBER === "1") && hero) {
  import("./scrubber.js").then(({ mountScrubber }) => mountScrubber(hero));
}

// The document's copy comes in as its act comes into view, once
// (landing.css). Acts already in view are marked before the copy is hidden,
// so nothing on screen blinks.
function watchActArrivals() {
  const acts = [...document.querySelectorAll("[data-act]")];
  const arrive = (act) => { act.dataset.arrived = ""; };
  if (typeof IntersectionObserver !== "function") {
    acts.forEach(arrive);
    return;
  }
  const line = innerHeight * 0.9;
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
  }, { rootMargin: "0px 0px -10% 0px" });
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
  // The hero was laid out for the film; it rests in the document's layout.
  hero?.finish("document");
  delete root.dataset.mode;
  delete root.dataset.stage;
  // A call to action pressed while the film was pending still goes where it
  // pointed, now in the document.
  if (/^#act-\d+$/.test(location.hash)) document.getElementById(location.hash.slice(1))?.scrollIntoView();
}

if (root.dataset.mode === "film") {
  import("../../film/film.js")
    .then(({ startFilm }) => {
      if (root.dataset.mode !== "film") return;
      root.dataset.stage = "starting";
      startFilm({ ignoreFrameBudget: requested === "force", hero });
    })
    .catch(backToDocument);
}
