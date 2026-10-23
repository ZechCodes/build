import { BUILD_CHANNEL } from "../config.js";

/** Build identity lives outside route content and is set before the gate boots. */
export function initBuildChannel() {
  if (BUILD_CHANNEL !== "dev") return;
  document.getElementById("build-channel").hidden = false;
  document.title = "Build Dev — review is the product";
}
