# Temporary #333 hero preview

`/lab/hero-7c92e4b1a630/` is an unlisted copy of the home page with the hero
from `149bf3c35179c83a041348b8f29ec6f4e94f49d6`. The field, entrance, timing,
CSS and supporting hero modules here are copied from that revision. The
page copies its full story so the film and document layouts match. The lab
route serves static HTML, so its GitHub link is filled here and its activity
slot is empty. It uses the existing lab route's noindex response header,
adds noindex/nofollow metadata, and is linked from neither public page.

This duplication is temporary and intentionally isolated. Gating the new
markup, field generation, spacing, ripple and short-phone timing throughout
the shared hero would be harder to remove and verify. `preview.css` bundles
its own cascade. `build.mjs` builds its script graph separately after Astro,
so the new entry cannot split or rewrite the existing home and lab bundles.
The generated-page test pins all bytes emitted by main at `41c2861e`.

`hero333-boot.js` ignores the played memory on every load; the copied entrance
does not write it either. Reduced motion, `?hero=0` and links to later acts
retain their normal behavior. The home page's boot and hero sources stay as
they were. Remove this directory, page, boot, build integration and snapshot
checks when #333 is accepted as the default.
