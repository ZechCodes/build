// @vitest-environment jsdom
// The one sentence this client says about a machine it cannot reach, and the
// markup a work surface stands up in place of itself while that is true. Every
// surface that names a missing machine — a branch, an issue, a sheet the
// toolbar refuses to open — says it in these words.

import { beforeEach, describe, expect, it } from "vitest";

import { App } from "../src/app.js";
import { deviceOfflineHtml, deviceOfflineNotice } from "../src/core/deviceNotice.js";

beforeEach(() => {
  App.devices = [{ id: "dev-1", name: "workshop", status: "offline" }];
});

describe("what a client says about a machine it cannot reach", () => {
  it("names the device the way the account does", () => {
    expect(deviceOfflineNotice("dev-1")).toBe("workshop isn't connected, so this can't be opened right now.");
  });

  it("falls back to plain words for a device the account has never listed", () => {
    expect(deviceOfflineNotice("dev-unknown")).toContain("That device isn't connected");
  });

  it("stands the same sentence up as a surface's empty state, with the name escaped", () => {
    App.devices = [{ id: "dev-1", name: "<script>", status: "offline" }];

    const html = deviceOfflineHtml("dev-1");

    expect(html).toContain('<div class="empty">');
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });
});
