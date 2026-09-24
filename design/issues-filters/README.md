# The Issues tab's filter bar, after #44

The maintainer, 2026-09-20: "Would be nice if we used custom drop downs so it would be
possible to select multiple and have fuzzy search for labels and assignees."

- `menu-390-dark.png` — a phone. The popover is a sheet at the bottom of the
  screen, where a thumb is, rather than a menu hanging off wherever the bar
  happened to wrap the press to. Two labels ticked; the press above says
  `Labels · 2`.
- `menu-1440-dark.png` — the assignee menu, grouped by the workspace each agent
  stands on, the way the assignee picker groups them. Two chosen; the press
  says `tracker-filters · Agent 1 +1`, because the first name is the most
  useful word on the bar.
- `menu-1440-light-search.png` — the same control searching. `tr` finds
  `tracker` and `transport`: it is `core/fuzzy.js`'s subsequence rank, which
  the branch picker already uses.

All four filters wear this control — State and Column in single-select mode —
so the bar reads as one family rather than as two native selects beside two of
ours.

## Making them again

Drawn by mounting the REAL `mountIssuesChrome` against the REAL sheet, so the
picture is of the code:

```sh
node design/issues-filters/make-pages.mjs      # writes spa/__menu-*.html
cd spa && npx vite --port 4188 --strictPort &  # icons.js needs vite to resolve
node ../design/issues-filters/shoot.mjs        # writes /tmp/rowshot/menu-*.png
```

Delete the `spa/__menu-*.html` pages afterwards; they are scaffolding, not
fixtures. `shoot.mjs` uses web/'s Playwright (run `npm install` in `web/`
first) against the system Chromium at `/usr/bin/chromium`.
