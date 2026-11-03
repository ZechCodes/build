// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { installReviewLinkNavigation } from "../src/core/reviewLinkNavigation.js";
import { hashFromRoute } from "../src/core/router.js";

const scope = { deviceId: "d", projectId: "p", taskId: "t" };
const route = { name: "trackerTask", ...scope };
let cleanup;
afterEach(() => { cleanup?.(); document.body.innerHTML = ""; });
function mount(target = route) {
  history.replaceState(null, "", hashFromRoute(route));
  document.body.innerHTML = '<a class="md-ref">Link</a>';
  const link = document.querySelector("a");
  link.href = hashFromRoute({ ...target, review: { revision: 1, mode: "all", directoryId: "dir", path: "a.js" } });
  const open = vi.fn();
  cleanup = installReviewLinkNavigation(scope, open);
  return { link, open };
}
const click = (link, options = {}) => {
  const event = new MouseEvent("click", { bubbles: true, cancelable: true, ...options });
  link.dispatchEvent(event);
  return event;
};
it("leaves other tasks, devices, modified clicks and embedded workspace tasks to normal navigation", () => {
  for (const target of [{ ...route, taskId: "other" }, { ...route, deviceId: "other" }]) {
    const { link, open } = mount(target);
    expect(click(link).defaultPrevented).toBe(false);
    expect(open).not.toHaveBeenCalled();
    cleanup();
  }
  const { link, open } = mount();
  expect(click(link, { ctrlKey: true }).defaultPrevented).toBe(false);
  history.replaceState(null, "", "#/device/d/project/p/workspace/w/tasks/t");
  expect(click(link).defaultPrevented).toBe(false);
  expect(open).not.toHaveBeenCalled();
});
it("cleans up the document listener on disposal", () => {
  const { link, open } = mount();
  cleanup();
  expect(click(link).defaultPrevented).toBe(false);
  expect(open).not.toHaveBeenCalled();
});
