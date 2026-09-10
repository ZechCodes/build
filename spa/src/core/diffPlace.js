import { patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { FILE_ELEMENT } from "./diffRender.js";
import { anchorTop } from "./paintKeepingPlace.js";

export const DIFF_PLACE_KEEPING = {
  opening: (scroller) => !scroller.querySelector(FILE_ELEMENT),
  policy: anchorTop(FILE_ELEMENT),
};

const CHANGESET_PARTS = '<div class="csbar"></div><div class="dstack" data-keyed-list></div><div class="cstray"></div>';

const PART_CLASSES = ["csbar", "dstack", "cstray"];

export function createChangesetPaint(host) {
  const standingParts = () => PART_CLASSES.map((name) => [...host.children].find((child) => child.classList.contains(name)));
  return ({ bar, entries, tray }) => {
    let [barPart, stackPart, trayPart] = standingParts();
    if (!barPart || !stackPart || !trayPart) {
      host.innerHTML = CHANGESET_PARTS;
      [barPart, stackPart, trayPart] = standingParts();
    }
    patchInnerHtml(barPart, bar);
    patchList(stackPart, entries, {
      keyOf: (entry) => entry.key,
      render: (entry) => entry.html,
      signatureOf: (entry) => entry.html,
    });
    patchInnerHtml(trayPart, tray);
  };
}
