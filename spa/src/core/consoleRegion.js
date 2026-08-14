// The console: the collapsible bottom panel that will host the selected
// branch's or issue's terminal tabs. This is the reserved slot — the bar, at
// the bottom of the view column, collapsed. The panel, its sizes and the
// terminals inside it arrive with the console item; until then the bar says
// what it is and stays shut.

import "../styles/shell.css";

export function consoleBarHtml() {
  return `<button class="console-bar" id="console-toggle" aria-expanded="false" disabled
      title="Terminals move here"><span class="console-caret">▲</span><span>Console</span></button>`;
}

/** Paint the collapsed bar into the shell's console region. */
export function mountConsoleRegion(host) {
  if (!host) return;
  host.innerHTML = consoleBarHtml();
}
