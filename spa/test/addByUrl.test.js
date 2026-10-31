// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { openAddByUrl } from "../src/sheets/addByUrl.js";

const identity = { deviceId: "device", name: "Laptop <script>", fingerprint: "fingerprint", origin: "https://bridge.example", endpointEpoch: "epoch", familyId: "family" };
const flush = async () => { for (let i = 0; i < 15; i += 1) await Promise.resolve(); };
beforeEach(() => { document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>'; });
const services = () => ({ begin: vi.fn().mockResolvedValue({ identity }), complete: vi.fn().mockResolvedValue(identity), connect: vi.fn(), discard: vi.fn(), navigate: vi.fn() });

describe("Add by URL sheet", () => {
  it("shows verified identity and requires explicit HTTPS selection", async () => {
    const api = services(); const done = vi.fn();
    openAddByUrl(done, { services: api });
    document.querySelector("#bridge-url").value = identity.origin;
    document.querySelector("#urlverify").click(); await flush();
    expect(document.querySelector("#sheet").textContent).toContain(identity.name);
    expect(document.querySelector("#sheet script")).toBeNull();
    expect(api.connect).not.toHaveBeenCalled();
    document.querySelector("#urlconnect").click(); await flush();
    expect(api.connect).toHaveBeenCalledWith(identity);
    expect(done).toHaveBeenCalledOnce();
    expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
  });

  it("navigates only to the hosted resume document without probing again", async () => {
    const api = services(); api.begin.mockResolvedValue({ navigateTo: "https://app.example/app/?bridge_discovery=id" });
    openAddByUrl(vi.fn(), { services: api });
    document.querySelector("#bridge-url").value = identity.origin;
    document.querySelector("#urlverify").click(); await flush();
    expect(api.navigate).toHaveBeenCalledWith("https://app.example/app/?bridge_discovery=id");
    expect(api.complete).not.toHaveBeenCalled();
  });

  it("discards a late discovery after cancellation without touching a successor sheet", async () => {
    const api = services(); let resolve;
    api.begin.mockImplementation(() => new Promise((done) => { resolve = done; }));
    openAddByUrl(vi.fn(), { services: api });
    document.querySelector("#bridge-url").value = identity.origin;
    document.querySelector("#urlverify").click();
    document.querySelector("#urlcancel").click();
    document.querySelector("#sheet").textContent = "Successor";
    resolve({ identity }); await flush();
    expect(api.discard).toHaveBeenCalledWith(identity);
    expect(document.querySelector("#sheet").textContent).toBe("Successor");
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("leaves authorization failures visible and does not choose WebRTC implicitly", async () => {
    const api = services(); api.complete.mockRejectedValue(Object.assign(Error("Authorization refused"), { code: "authentication" }));
    openAddByUrl(vi.fn(), { challengeId: "id", services: api }); await flush();
    expect(document.querySelector("#urlerror").textContent).toContain("authorization");
    expect(api.connect).not.toHaveBeenCalled();
  });
});
