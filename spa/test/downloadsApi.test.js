// The download links come from the api, never from the client: the api knows
// the releases repo and the public origin, and the SPA renders what it is told.
// Its refusals reach the human as the api's own sentence — "invite only" while
// the alpha is closed — and as one plain fallback while the route is still 404.

import { describe, expect, it, vi } from "vitest";
import { fetchDownloads } from "../src/api.js";

const PAYLOAD = {
  install_command: "curl -fsSL https://getbuild.ing/install.sh | sh",
  install_script_url: "https://getbuild.ing/install.sh",
  releases_url: "https://github.com/ZechCodes/build-releases/releases/latest",
  checksums_url: "https://github.com/ZechCodes/build-releases/releases/latest/download/SHA256SUMS",
  platforms: [
    {
      key: "macos-arm64",
      label: "macOS · Apple silicon",
      url: "https://github.com/ZechCodes/build-releases/releases/latest/download/build-bridge-macos-arm64.tar.gz",
    },
  ],
};

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
