// A reusable GitHub-style split button: a primary (default) action plus a caret
// that opens a menu of the remaining actions. Generalizes the git split button
// once inlined in views/task.js. Pure markup + thin wiring; every user-supplied
// string is escaped.

import { esc } from "./text.js";

/** Pure markup for a GitHub-style split button. options[0] is the default.
 *  option: { id, label, menuLabel?, description, busyLabel, danger? }
 *  With one option: a plain button, no caret, no menu. All strings escaped. */
export function splitButtonMarkup(options) {
  const primary = options[0];
  const dangerClass = primary.danger ? " danger" : "";
  const primaryButton = `<button class="btn primary${dangerClass}" data-action="${esc(primary.id)}">${esc(primary.label ?? primary.menuLabel)}</button>`;
  if (options.length === 1) return `<div class="splitbtn">${primaryButton}</div>`;
  const items = options
    .map(
      (o) =>
        `<div class="mi" data-action="${esc(o.id)}"><span class="mt">${esc(o.menuLabel ?? o.label)}</span><span class="md">${esc(o.description)}</span></div>`,
    )
    .join("");
  return `<div class="splitbtn">${primaryButton}<button class="btn primary${dangerClass} caret" title="More actions">▾</button><div class="splitmenu" hidden>${items}</div></div>`;
}

/** Pure single-flight latch: begin() arms and returns true, or returns false
 *  when a flight is already in progress; end() re-arms; active() peeks at the
 *  latch (callers freeze repaints while a flight is running). */
export function createSingleFlight() {
  let inFlight = false;
  return {
    begin() {
      if (inFlight) return false;
      inFlight = true;
      return true;
    },
    end() {
      inFlight = false;
    },
    active() {
      return inFlight;
    },
  };
}

/** Render into `container` and wire behavior. `run(optionId)` is awaited; while
 *  in flight the primary button and caret are disabled, the menu stays closed,
 *  and further invokes are ignored (single flight — no concurrent destructive
 *  RPCs); rejection restores label + enabled (the caller owns error display);
 *  resolution leaves both disabled (the caller repaints/navigates). The caret
 *  toggles the menu; a pointerdown outside closes it; a menu item runs its
 *  option through the same primary button. Callers whose surface remounts the
 *  button while an action can be pending (poll-driven repaints) pass a shared
 *  `flight` latch so a remount can never re-arm a fresh one mid-flight. */
export function mountSplitButton(container, { options, run, flight = createSingleFlight() }) {
  container.innerHTML = splitButtonMarkup(options);
  const byId = Object.fromEntries(options.map((o) => [o.id, o]));
  const primary = container.querySelector(".btn.primary:not(.caret)");
  const caret = container.querySelector(".caret");
  const menu = container.querySelector(".splitmenu");

  const invoke = async (optionId) => {
    if (!flight.begin()) return;
    const option = byId[optionId];
    if (menu) menu.hidden = true;
    primary.disabled = true;
    if (caret) caret.disabled = true;
    const restoreLabel = primary.textContent;
    primary.textContent = option.busyLabel || "working…";
    try {
      await run(optionId);
      // Resolution: leave primary + caret disabled — the caller repaints or navigates.
    } catch {
      // Rejection: the caller has shown the error; restore the buttons so the
      // user can retry a different action.
      primary.disabled = false;
      if (caret) caret.disabled = false;
      primary.textContent = restoreLabel;
    } finally {
      flight.end();
    }
  };

  primary.onclick = () => invoke(primary.dataset.action);

  if (caret && menu) {
    caret.onclick = (event) => {
      event.stopPropagation();
      if (caret.disabled) return;
      menu.hidden = !menu.hidden;
      if (!menu.hidden) {
        const close = (ev) => {
          if (!container.querySelector(".splitbtn")?.contains(ev.target)) {
            menu.hidden = true;
            document.removeEventListener("pointerdown", close);
          }
        };
        setTimeout(() => document.addEventListener("pointerdown", close), 0);
      }
    };
    menu.querySelectorAll(".mi").forEach(
      (mi) =>
        (mi.onclick = () => {
          menu.hidden = true;
          invoke(mi.dataset.action);
        }),
    );
  }
}
