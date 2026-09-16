// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { openSetRemote } from "../src/sheets/setRemote.js";

beforeEach(() => {
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

it("keeps its title outside the scrolling settings body", () => {
  openSetRemote({ project_id: "p1", name: "Build", remote: "" }, vi.fn(), { callRpc: vi.fn() });
  const frame = document.querySelector("#sheet > .settings-sheet-frame");
  expect(frame.querySelector(":scope > .settings-sheet-header h3").textContent).toBe("Set remote");
  expect(frame.querySelector(":scope > .settings-sheet-body #srurl")).not.toBeNull();
  expect(document.activeElement).toBe(document.querySelector("#srurl"));
});
