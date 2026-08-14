// The inbox route's centre column. The inbox itself is the left rail — global
// and persistent — so this surface is what stands beside it before a work item
// is chosen. The rail's entries, their states and the Done control land here
// with the inbox item; this is the shell's landing paint.

import { $ } from "../dom.js";
import "../styles/shell.css";

export function renderInbox() {
  const root = $("#root");
  root.className = "surface";
  root.innerHTML = `
    <div class="shell-stub">
      <h2>Nothing open</h2>
      <p>Pick a branch or an issue from the inbox to see its changes, or start something new.</p>
    </div>`;
}
