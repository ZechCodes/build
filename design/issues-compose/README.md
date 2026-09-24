# Filing an issue in place, after #57

The maintainer: "Issue creation should be inline and support file attachments."

- `compose-390-dark.png` — the phone. The composer is a sheet at the foot of
  the screen, like the filter menus, with the list it is being filed against
  still behind it. Two real attachments in the tray: a thumbnail for the image,
  a glyph for the log.
- `compose-1440-dark.png` — the same form at the head of the list, with #45
  visible under it. Nothing is covered and nothing navigated.

Column, Priority and Labels are the same control as the filter bar above them.
The assignee keeps its own, because two of its five kinds open forms and one of
them cuts a checkout — a pick-one-from-a-list cannot say "file this and start
an agent on a new workspace".

## Making them again

Drawn by mounting the REAL chrome, the REAL keyed rows and the REAL composer
against the REAL sheet, and by dropping REAL files through the tray, so the
chips are the code's rather than a drawing of them.

```sh
node design/issues-compose/make-pages.mjs      # writes spa/__compose-*.html
cd spa && npx vite --port 4188 --strictPort &  # icons.js needs vite to resolve
node ../design/issues-compose/shoot.mjs        # writes /tmp/rowshot/compose-*.png
```

Delete the `spa/__compose-*.html` pages afterwards; they are scaffolding, not
fixtures.
