// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { mountFileContextMenu } from "../src/core/fileContextMenu.js";

let menu;
afterEach(() => { menu?.dispose(); document.body.innerHTML = ""; vi.useRealTimers(); });
const setup = () => {
  document.body.innerHTML = '<div id="host"><div class="frow" data-kind="file" data-path="a.txt" tabindex="0">a.txt</div></div>';
  const host = document.querySelector("#host");
  const row = host.firstElementChild;
  const choose = vi.fn();
  menu = mountFileContextMenu(host, { itemsFor: () => [{ id: "download", label: "Download" }], onChoose: choose });
  return { row, choose };
};
const open = (row) => row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 30 }));
const key = (row, name, props = {}) => row.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...props }));
it("opens on right click, chooses the captured row, and removes the menu", () => {
  const { row, choose } = setup();
  expect(open(row)).toBe(false);
  const item = document.querySelector('[role="menuitem"]');
  expect(item.textContent).toContain("Download");
  item.click();
  expect(choose).toHaveBeenCalledWith("download", row);
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
it.each([["ContextMenu", {}], ["F10", { shiftKey: true }]])("opens with %s and returns focus on Escape", (name, props) => {
  const { row } = setup();
  row.focus();
  key(row, name, props);
  expect(document.activeElement.getAttribute("role")).toBe("menuitem");
  key(document.activeElement, "Escape");
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(row);
});
it("dismisses on an outside click and disposal", () => {
  const { row } = setup();
  open(row);
  document.body.click();
  expect(document.querySelector('[role="menu"]')).toBeNull();
  open(row);
  menu.dispose();
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
it("closes when a cache repaint removes the menu's row", async () => {
  const { row } = setup();
  open(row);
  row.remove();
  await vi.waitFor(() => expect(document.querySelector('[role="menu"]')).toBeNull());
});
