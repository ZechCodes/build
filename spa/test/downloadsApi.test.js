// The download links come from the api, never from the client: the api knows
// where the assets live and what the public origin is, and the SPA renders what
// it is told. Its refusals reach the human as the api's own sentence — "invite
// only" while the alpha is closed — and as one plain fallback otherwise.
//
// The install one-liner carries a download token that lives ten minutes, so the
// SPA can ask for a fresh one; that mint is a POST of its own.

import { describe, expect, it, vi } from "vitest";
import { fetchDownloads, mintInstallCommand } from "../src/api.js";
import { downloadsPayload, mintedCommand } from "./downloadsFixture.js";

const PAYLOAD = downloadsPayload();

function answering(status, body) {
  return vi.fn(async () => ({
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  }));
}

describe("fetchDownloads", () => {
  it("GETs /app/downloads and returns the payload verbatim", async () => {
    globalThis.fetch = answering(200, PAYLOAD);
    await expect(fetchDownloads()).resolves.toEqual(PAYLOAD);
    expect(globalThis.fetch).toHaveBeenCalledWith("/app/downloads");
  });

  it("surfaces the api's own refusal when the account is not in the alpha", async () => {
    globalThis.fetch = answering(403, { status_code: 403, detail: "invite only" });
    await expect(fetchDownloads()).rejects.toThrow("invite only");
  });

  it("falls back to one plain sentence when the route answers nothing useful", async () => {
    globalThis.fetch = answering(404, "");
    await expect(fetchDownloads()).rejects.toThrow("downloads are not available");
  });
});

describe("mintInstallCommand", () => {
  it("POSTs /app/downloads/token and returns the line the api minted", async () => {
    globalThis.fetch = answering(201, { token: "dl_x", install_command: mintedCommand(), expires_in_s: 600 });
    await expect(mintInstallCommand()).resolves.toEqual({
      token: "dl_x",
      install_command: mintedCommand(),
      expires_in_s: 600,
    });
    expect(globalThis.fetch).toHaveBeenCalledWith("/app/downloads/token", { method: "POST" });
  });

  it("surfaces the api's own refusal when the invite was revoked mid-session", async () => {
    globalThis.fetch = answering(403, { status_code: 403, detail: "invite only" });
    await expect(mintInstallCommand()).rejects.toThrow("invite only");
  });

  it("falls back to one plain sentence when the route answers nothing useful", async () => {
    globalThis.fetch = answering(500, "");
    await expect(mintInstallCommand()).rejects.toThrow("could not refresh the install line");
  });
});
