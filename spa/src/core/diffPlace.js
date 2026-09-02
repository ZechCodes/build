// Painting a changeset, and leaving the reader where they were.
//
// Both surfaces that draw a diff — the Changes pane and the aggregate review
// plug — draw the same three things into the same shape of host: a bar over the
// stack, the keyed stack itself, and the tray the surface's own verbs live in.
// Each is patched in its own place, so a tick that changed one file writes one
// file and the reader keeps their scroll, their selection, and whatever they
// had open.

import { patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { FILE_ELEMENT } from "./diffRender.js";
import { anchorTop } from "./paintKeepingPlace.js";

/** Where a repaint of a stack leaves the reader: on the file they were reading. */
export const DIFF_PLACE_KEEPING = {
  opening: (scroller) => !scroller.querySelector(FILE_ELEMENT),
  policy: anchorTop(FILE_ELEMENT),
};

const CHANGESET_PARTS = '<div class="csbar"></div><div class="dstack" data-keyed-list></div><div class="cstray"></div>';

const PART_CLASSES = ["csbar", "dstack", "cstray"];

/** Paint changesets into `host`: `paint({ bar, entries, tray })`.
 *
 *  The parts are looked up as `host`'s own children and made again unless all
 *  three are standing — the two surfaces share this host, and whichever of them
 *  wrote there last may have left something else entirely. */
export function createChangesetPaint(host) {
  const parts = () => PART_CLASSES.map((name) => [...host.children].find((child) => child.classList.contains(name)));
  return ({ bar, entries, tray }) => {
    let [barPart, stackPart, trayPart] = parts();
    if (!barPart || !stackPart || !trayPart) {
      host.innerHTML = CHANGESET_PARTS;
      [barPart, stackPart, trayPart] = parts();
    }
    patchInnerHtml(barPart, bar);
    patchList(stackPart, entries, { keyOf: (entry) => entry.key, render: (entry) => entry.html });
    patchInnerHtml(trayPart, tray);
  };
}
