// The account pages, reached from the foot of the inbox rail: settings (which
// holds the paired devices) and the archive of finished work. One route family,
// one reading column, with the pages named above whichever one is open.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { accountNavHtml } from "../core/archive.js";
import { renderSettings } from "./settings.js";
import { renderArchive } from "./archive.js";
import "../styles/shell.css";

/** Name the account's pages above the one that is open. The page has already
 *  claimed #root, so the nav goes in ahead of it — a page never has to know it
 *  is one of several. */
function mountAccountNav(page) {
  const root = $("#root");
  if (!root) return;
  root.insertAdjacentHTML("afterbegin", accountNavHtml(page));
  root.querySelectorAll(".account-nav .t").forEach((tab) => {
    const open = () => go({ name: "account", page: tab.dataset.page });
    tab.onclick = open;
    tab.onkeydown = (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      open();
    };
  });
}

export async function renderAccount() {
  // Devices live inside the settings page today; anything that is not the
  // archive lands there.
  const page = App.route.page === "archive" ? "archive" : "settings";
  if (page === "archive") renderArchive();
  else await renderSettings();
  mountAccountNav(page);
}
