import { BUILD_CHANNEL } from "../config.js";

/** Set the build's tab identity before the gate boots. */
export function initBuildChannel() {
  if (BUILD_CHANNEL === "dev") document.title = "Build Dev — review is the product";
}

/** Build identity belongs to the connection button and follows its lifetime. */
export function mountBuildChannel(button) {
  if (BUILD_CHANNEL !== "dev") return;
  const bubble = button.ownerDocument.createElement("span");
  bubble.className = "connection-build-channel";
  bubble.setAttribute("aria-hidden", "true");
  bubble.textContent = "D";
  button.append(bubble);
  button.setAttribute("aria-description", "Dev build");
}
