// Arrange real refusals without changing a server response, credential or clock.
// The real account issuer revokes one family; eight actual HTTP streams
// fill another family before the SPA attempts its ninth stream.
import assert from "node:assert/strict";

export async function refusalFixture(context, fixture) {
  let next;
  const held = [];
  const json = async (mint, path, body, session) => {
    const response = await fetch(fixture.bridgeOrigin + path, {
      method: "POST", signal: AbortSignal.timeout(5000),
      headers: { Origin: fixture.hostedOrigin, Authorization: `Bearer ${mint.access_token}`,
        "Content-Type": "application/json", ...(session ? { "X-Build-Session": session } : {}) },
      body: JSON.stringify(body),
    });
    assert.equal(response.status, 201);
    return response.json();
  };
  const holdEight = async (mint) => {
    for (let index = 0; index < 8; index++) {
      const opening = await json(mint, "/v1/sessions", { api_range: "^3.0.0" });
      const subscription = await json(mint, "/v1/subscriptions", { kind: "application" }, opening.session_id);
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 5000);
      try {
        const response = await fetch(fixture.bridgeOrigin + subscription.events_path, {
          signal: controller.signal, headers: { Origin: fixture.hostedOrigin,
            Authorization: `Bearer ${mint.access_token}`, "X-Build-Session": opening.session_id, Accept: "text/event-stream" },
        });
        assert.equal(response.status, 200);
        const reader = response.body.getReader();
        held.push({ reader, controller, session: opening.session_id, token: mint.access_token });
        let hello = "";
        while (!hello.includes("\n\n")) {
          const chunk = await reader.read();
          assert.equal(chunk.done, false);
          hello += new TextDecoder().decode(chunk.value);
          assert.ok(hello.length < 65536);
        }
        assert.match(hello, /^event: hello\n/);
      } finally { clearTimeout(timer); }
    }
  };
  await context.route("**/api/bridge-discovery/complete", async (route) => {
    const mode = next;
    next = null;
    const response = await route.fetch();
    if (mode && response.ok()) {
      const mint = await response.json();
      if (mode === "revoked") {
        // Families are bound to the issuing browser login, independently of
        // the same account's desktop OAuth login. Preserve that real binding.
        const revoked = await context.request.delete(`${fixture.hostedOrigin}/api/devices/${mint.device_id}/http-credentials/${mint.family_id}`);
        assert.equal(revoked.status(), 204, "the real issuer revoked the disposable family");
      } else {
        await holdEight(mint);
      }
    }
    await route.fulfill({ response });
  });
  return {
    arm(mode) { assert.ok(["revoked", "ninth"].includes(mode)); next = mode; },
    async close() {
      await Promise.allSettled(held.splice(0).map(async ({ reader, controller, session, token }) => {
        controller.abort();
        await reader.cancel().catch(() => {});
        await fetch(`${fixture.bridgeOrigin}/v1/sessions/${session}`, { method: "DELETE", signal: AbortSignal.timeout(1000),
          headers: { Origin: fixture.hostedOrigin, Authorization: `Bearer ${token}` } });
      }));
    },
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
    await page.screenshot({ path: `${output}/spa-${mode}-refusal.png`, fullPage: true });
    evidence.push({ case: mode, status: refused.status, message });
    console.log(`PASS SPA ${mode} refusal: HTTP ${refused.status}; ${message}`);
    await page.click("#urlcancel");
    await arranged.close();
    await page.selectOption("#device-transport", "webrtc");
    await page.waitForFunction(() => !document.querySelector("#device-transport")?.disabled);
  }
  return evidence;
}
