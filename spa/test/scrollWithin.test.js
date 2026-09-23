/** @vitest-environment jsdom */
import { expect, it, vi } from "vitest";
import { scrollWithin } from "../src/core/scrollWithin.js";

it("scrolls only the named container to the target's offset", () => {
  document.body.innerHTML = '<div id="shell"><div id="pane"><div id="line"></div></div></div>';
  const shell = document.querySelector("#shell");
  const pane = document.querySelector("#pane");
  const line = document.querySelector("#line");
  shell.scrollTo = vi.fn();
  pane.scrollTo = vi.fn();
  document.documentElement.scrollTo = vi.fn();
  pane.scrollTop = 25;
  Object.defineProperty(pane, "clientTop", { value: 2 });
  vi.spyOn(pane, "getBoundingClientRect").mockReturnValue({ top: 100 });
  vi.spyOn(line, "getBoundingClientRect").mockReturnValue({ top: 450, height: 20 });

  scrollWithin(pane, line);

  expect(pane.scrollTo).toHaveBeenCalledWith({ top: 373, behavior: "smooth" });
  expect(shell.scrollTo).not.toHaveBeenCalled();
  expect(document.documentElement.scrollTo).not.toHaveBeenCalled();
});
