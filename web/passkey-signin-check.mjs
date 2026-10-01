// Create an account and sign in on the real sign-in page (#312) in headless Chromium,
// with a CDP virtual authenticator, while navigator.credentials behaves like Safari:
// one request at a time, a second answered "A request is already pending.", and an
// aborted request released only when its rejection lands. Chromium itself cancels the
// pending autofill request for you, which is why the bug never showed there.
//
// Start a local app whose passkey method's redirect_base_url matches SIGNIN_URL (see
// skriftapp/README.md for the dev app; set `auth.methods.passkey.type: passkey`), then:
//
//   SIGNIN_URL=http://localhost:8391 node web/passkey-signin-check.mjs
//
// Exit 0 when both ceremonies reach a signed-in redirect and the page asks nothing of
// another origin. Never point it at production: it creates accounts.
import assert from "node:assert/strict";
import { chromium } from "playwright";

const base = process.env.SIGNIN_URL || "http://localhost:8391";
const email = `signin-check-${Date.now()}@example.com`;

// Runs in the page before any of its scripts.
function behaveLikeSafari() {
  const credentials = navigator.credentials;
  const native = { get: credentials.get.bind(credentials), create: credentials.create.bind(credentials) };
  let pending = false;
  window.__webauthnLog = [];
  // Skrift asks for no particular residentKey, and the virtual authenticator then
  // makes a credential only the server's allowCredentials can find. The platform
  // authenticators people use (iCloud Keychain, Google Password Manager, Windows
  // Hello) always make discoverable passkeys, so ask for one, as they would.
  const discoverable = (options) =>
    options.publicKey
      ? { ...options, publicKey: { ...options.publicKey, authenticatorSelection: { residentKey: "required" } } }
      : options;
  for (const kind of ["get", "create"]) {
    credentials[kind] = (options = {}) => {
      if (kind === "create") options = discoverable(options);
      const entry = `${kind}:${options.mediation || "modal"}`;
      window.__webauthnLog.push(entry);
      console.log(`webauthn ${entry}`);
      if (pending) return Promise.reject(new DOMException("A request is already pending.", "InvalidStateError"));
      pending = true;
      const release = (settle) => setTimeout(() => ((pending = false), settle()), 50);
      // Autofill stays open, as it does while nobody opens the email field's menu,
      // until the page aborts it; then it is let go of a moment later.
      if (options.mediation === "conditional") {
        return new Promise((resolve, reject) => {
          const aborted = () => release(() => reject(new DOMException("Aborted.", "AbortError")));
          if (options.signal?.aborted) aborted();
          else options.signal?.addEventListener("abort", aborted);
        });
      }
      return native[kind](options).then(
        (value) => ((pending = false), value),
        (error) => new Promise((resolve, reject) => release(() => reject(error))),
      );
    };
  }
}

const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH });
const failures = [];
try {
  const context = await browser.newContext();
  await context.addInitScript(behaveLikeSafari);
  const page = await context.newPage();
  const external = [];
  page.on("console", (message) => {
    if (message.text().startsWith("webauthn ")) console.log(`  ${message.text()}`);
  });
  page.on("request", (request) => {
    if (!request.url().startsWith(base)) external.push(request.url());
  });
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });

  async function ceremony(name, act, statusSelector) {
    await page.goto(`${base}/auth/passkey/login`);
    await page.waitForFunction(() => window.__webauthnLog.includes("get:conditional"), null, { timeout: 10_000 });
    await act();
    try {
      await page.waitForURL((url) => !url.pathname.startsWith("/auth/"), { timeout: 10_000 });
      console.log(`${name}: signed in → ${page.url()}`);
    } catch {
      const status = await page.locator(statusSelector).textContent().catch(() => "(no status line)");
      failures.push(`${name}: still on the sign-in page — "${status}"`);
    }
  }

  await ceremony(
    "create account",
    async () => {
      const form = page.locator("#signup-form, #passkey-signup-form").first();
      await form.locator('input[name="email"]').fill(email);
      await form.locator('button[type="submit"]').click();
    },
    "#signup-status, #passkey-signup-status",
  );
  await context.clearCookies();
  await ceremony(
    "sign in",
    () => page.locator("#signin-form, #passkey-login-form").first().locator('button[type="submit"]').click(),
    "#signin-status, #passkey-login-status",
  );
  if (external.length) failures.push(`requests to another origin: ${external.join(", ")}`);
} finally {
  await browser.close();
}

assert.deepEqual(failures, [], failures.join("\n"));
console.log("ok");
