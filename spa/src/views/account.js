// The account pages, reached from the foot of the inbox rail: settings, the
// paired devices, and the archive of finished work. One route family, one
// reading column.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { renderSettings } from "./settings.js";
import "../styles/shell.css";

export async function renderAccount() {
  // Devices live inside the settings page today; both pages land there until
  // the account item splits them.
  if (App.route.page === "archive") {
    $("#root").innerHTML = `
      <div class="board-head"><div><h1>Archive</h1><p>Work you marked done.</p></div></div>
      <div class="panel"><span class="dim" style="font-size:13px">The archive moves here from the project surfaces.</span></div>`;
    return;
  }
  await renderSettings();
}
