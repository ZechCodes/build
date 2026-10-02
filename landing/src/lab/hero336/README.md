# Temporary #336 notification wall preview

`/lab/wall-646fe5bc6ee6/` is an unlisted copy of the full landing story with
the proposed hero. The film and document layouts can be reviewed together.
The lab route serves static HTML, so its GitHub link is filled in the page and
its activity slot is empty. It has noindex/nofollow metadata and is linked
from neither the home page nor the existing notifications lab.

The hero implementation lives in this directory while the default home hero
continues to use `src/hero/`. `preview.css` takes the established landing,
film, and hero styles, then adds the wall treatment. `build.mjs` runs a
separate Vite build after Astro so preview code does not split or rewrite the
home page's bundle. The generated-page test pins the home page, notifications
lab, and their assets to the bytes emitted by main.

`hero336-boot.js` replays the entrance on every load without changing the
home page's played marker. Reduced motion, `?hero=0`, and links to later acts
keep their usual behavior. This is Step 1 only; acceptance would require a
separate decision to replace the default hero.
