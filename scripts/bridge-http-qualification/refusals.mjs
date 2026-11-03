// Arrange real refusals without changing a server response, credential or clock.
// The real account issuer revokes one family; eight actual native client
// streams fill another family before the SPA attempts its ninth stream.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createBridgeHttp } from "../../desktop/src/bridge-http.mjs";

export async function refusalFixture(context, fixture) {
  const manifest = JSON.parse(await readFile(new URL("../../desktop/src/bridge-http-resources.json", import.meta.url), "utf8"));
  const owner = { id: randomUUID() };
  let next;
  let ready = 0;
  const bridge = createBridgeHttp({
    hostedOrigin: fixture.hostedOrigin, manifest,
    getAuth: () => ({ generation: 1, accessToken: fixture.accessToken }),
    isOwnerCurrent: (candidate) => candidate === owner,
    send: (_recipient, event) => {
      if (event.event === "hello") ready++;
      if (event.event !== "reset_required") bridge.ack(owner, event.stream, event.sequence);
    },
  });
  const until = async (predicate) => {
    const deadline = Date.now() + 5000;
    while (!predicate()) {
      assert.ok(Date.now() < deadline, "all eight fixture streams receive real SSE hello");
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  };
  const holdEight = async (mint) => {
    const hint = { deviceId: mint.device_id, origin: mint.origin, endpointEpoch: mint.endpoint_epoch, familyId: mint.family_id };
    for (let index = 0; index < 8; index++) {
      const metadata = await bridge.resume(owner, hint);
      await bridge.open(owner, metadata.handle, "^3.0.0");
      await bridge.subscribe(owner, metadata.handle, { kind: "application" }, { requestId: randomUUID() });
      await until(() => ready === index + 1);
    }
  };
  await context.route("**/api/bridge-discovery/complete", async (route) => {
    const mode = next;
    next = null;
    const response = await route.fetch();
    if (mode && response.ok()) {
      const mint = await response.json();
      if (mode === "revoked") {
        const revoked = await fetch(`${fixture.hostedOrigin}/api/devices/${mint.device_id}/http-credentials/${mint.family_id}`, {
          method: "DELETE", headers: { Authorization: `Bearer ${fixture.accessToken}` },
        });
        assert.equal(revoked.status, 204, "the real issuer revoked the disposable family");
      } else {
        await holdEight(mint);
      }
    }
    await route.fulfill({ response });
  });
  return {
    arm(mode) { assert.ok(["revoked", "ninth"].includes(mode)); next = mode; ready = 0; },
    close() { bridge.clear(); },
  };
}

export async function qualifyBrowserRefusals(page, arranged, bridgeOrigin, output) {
  const evidence = [];
  for (const mode of ["revoked", "ninth"]) {
    arranged.arm(mode);
    const before = await page.evaluate(() => window.__qualification.requests.length);
    await page.click("[data-http-add]");
    await page.fill("#bridge-url", bridgeOrigin);
    await page.click("#urlverify");
    await page.locator("#urlconnect").waitFor({ timeout: 15000 });
    await page.click("#urlconnect");
    await page.waitForFunction(() => Boolean(document.querySelector("#urlerror")?.textContent.trim()), undefined, { timeout: 15000 });
    const message = await page.locator("#urlerror").innerText();
    const requests = await page.evaluate((start) => window.__qualification.requests.slice(start), before);
    const refused = requests.find((entry) => mode === "revoked"
      ? entry.path === "/v1/sessions" && entry.status === 401
      : entry.path.endsWith("/events") && entry.status === 429);
    assert.ok(refused, `${mode} refusal came from the real bridge`);
    assert.ok(message.length > 15, "a visible sentence explains the failed connection");
    assert.doesNotMatch(message, /[{}]|undefined|invalid_credential|invalid_stream/);
    if (mode === "revoked") assert.match(message, /authorization|Verify/i);
    await page.screenshot({ path: `${output}/spa-${mode}-refusal.png`, fullPage: true });
    evidence.push({ case: mode, status: refused.status, message });
    console.log(`PASS SPA ${mode} refusal: HTTP ${refused.status}; ${message}`);
    await page.click("#urlcancel");
    arranged.close();
    await page.selectOption("#device-transport", "webrtc");
    await page.waitForFunction(() => !document.querySelector("#device-transport")?.disabled);
  }
  return evidence;
}
