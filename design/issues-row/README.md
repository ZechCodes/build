# The Issues tab's list row, before and after #45

The maintainer, 2026-09-20: "The issue list still feels cluttered/messy."

`after-*.png` is the row this branch ships; `before-*.png` is the same dozen
issues on `760877d6`, which is the row #28 and #33 left behind. Two widths —
390 px and 1440 px — and both themes for the new one.

What changed, and nothing else did: the same facts in the same order, weighed
differently. Labels are small muted words spaced apart instead of a pill each,
three of them with `+n` for the rest. The column chip is the one chip on the
row. The priority left line two for a single mark before the title — `!` for
high, `!!` for urgent, nothing below that. Age and holder are dim. Rows are
separated by a hairline on a 12 px rhythm; the hover fill is gone, so the list
no longer flickers under a travelling pointer.

## Making them again

The pages are rendered by the REAL modules against the REAL sheet, so a
screenshot is of the code and not of a mock.

```sh
node design/issues-row/render-list.mjs "$PWD/spa" after     # this checkout
git archive <sha> spa/src | tar -x -C /tmp/old-spa          # …and any other
node design/issues-row/render-list.mjs /tmp/old-spa/spa before
node design/issues-row/shoot.mjs
```

`shoot.mjs` uses web/'s Playwright (run `npm install` in `web/` first) against
the system Chromium at `/usr/bin/chromium`. Both scripts write into
`/tmp/rowshot/`.
