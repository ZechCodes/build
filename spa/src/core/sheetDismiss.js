// Draft-protecting dismissal for the shared #sheet overlay. Scrim-click and
// Escape both route here: an empty sheet closes instantly, a sheet with typed
// content asks the reusable confirm modal before discarding. Pure predicate is
// unit-tested; the DOM layer stays thin.

import { $ } from "../dom.js";
import { confirmAction, isConfirmOpen } from "./confirm.js";

/** True when any field value survives trimming — i.e. there is a draft to lose. */
export function hasDraftValues(values) {
  return values.some((value) => typeof value === "string" && value.trim() !== "");
}

/** The editable text values currently in the sheet. Readonly inputs (e.g.
 *  newRepo's pre-filled Location) and non-text controls (checkboxes/radios) are
 *  excluded — they are not drafts the user would mourn. */
export function sheetDraftValues() {
  const sheet = document.getElementById("sheet");
  if (!sheet) return [];
  const fields = sheet.querySelectorAll(
    "textarea, input:not([readonly]):not([type=checkbox]):not([type=radio])",
  );
  return Array.from(fields).map((field) => field.value);
}

// Close the sheet by routing through its own Cancel control when present, so a
// sheet that settles a promise on cancel (implement.js) is not left with a
// stranded awaiter; otherwise just hide the scrim. Every sheet's Cancel button
// id ends in "cancel" and its handler removes the scrim's "show" class, so this
// is behaviour-consistent with an explicit Cancel click.
function closeSheet() {
  const cancel = document.querySelector("#sheet button[id$='cancel']");
  if (cancel) cancel.click();
  else {
    const scrim = document.getElementById("scrim");
    if (scrim) scrim.classList.remove("show");
  }
}

/** Dismiss the open sheet, protecting unsaved input. No-op while a confirm modal
 *  owns the screen (it handles its own Escape) or when no sheet is open. */
export function requestSheetDismiss() {
  if (isConfirmOpen()) return;
  const scrim = $("#scrim");
  if (!scrim || !scrim.classList.contains("show")) return;
  if (!hasDraftValues(sheetDraftValues())) {
    closeSheet();
    return;
  }
  confirmAction({
    title: "Discard this draft?",
    actions: ["Your typed text will be lost"],
    confirmLabel: "Discard",
    danger: true,
  }).then((discard) => {
    if (discard) closeSheet();
  });
}
