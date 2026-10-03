// Follow an invite link to account creation, then sign in, on the real sign-in page
// (#312, #314) in headless Chromium with a CDP virtual authenticator, while
// navigator.credentials behaves like Safari: one request at a time, a second answered
// "A request is already pending.".
//
// Start a local app whose passkey method's redirect_base_url matches SIGNIN_URL (see
// skriftapp/README.md for the dev app; set `auth.methods.passkey.type: passkey`), seed
// an open invite there as the README shows, then:
//
//   SIGNIN_URL=http://localhost:8391 INVITE_TOKEN=<raw token> node web/passkey-signin-check.mjs
//
// It checks that the page opens no WebAuthn request on its own, that without an invite
// it offers no account, that an invite visitor sees account creation alone and can switch
// to sign-in and back (#315), that an addressed invite locks its address or an open
// link asks for one, and that the email opt-in starts unchecked,
// that creating the account lands in /app/ as a member, and that the new passkey signs
// in. SCREENSHOT_DIR, when set, gets one PNG per state. Exit 0 when all of that holds
// and the page asks nothing of another origin. Never point it at production: it creates
// accounts and spends the invite.
import assert from "node:assert/strict";
import { chromium } from "playwright";

const base = process.env.SIGNIN_URL || "http://localhost:8391";
const inviteToken = process.env.INVITE_TOKEN;
const screenshots = process.env.SCREENSHOT_DIR;
assert.ok(inviteToken, "set INVITE_TOKEN to the raw token of an open invite on this app");

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
      return native[kind](options).finally(() => {
        pending = false;
      });
    };
  }
}

const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH });
const failures = [];
const check = (ok, failure) => ok || failures.push(failure);
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

  async function shoot(name) {
    if (screenshots) await page.screenshot({ path: `${screenshots}/${name}.png`, fullPage: true });
  }

  async function signedIn(name, statusSelector) {
    try {
      await page.waitForURL((url) => !url.pathname.startsWith("/auth/"), { timeout: 10_000 });
      console.log(`${name}: signed in → ${page.url()}`);
      return true;
    } catch {
      const status = await page.locator(statusSelector).textContent().catch(() => "(no status line)");
      failures.push(`${name}: still on the sign-in page — "${status}"`);
      return false;
    }
  }

  // No invite: sign-in only, and the way to the waitlist.
  await page.goto(`${base}/auth/login`);
  await page.waitForTimeout(500);
  check((await page.locator("form").count()) === 1, "no invite: the page shows more than the sign-in form");
  check((await page.locator("#signup-form").count()) === 0, "no invite: a create-account form was offered");
  check((await page.locator('a[href="/#waitlist"]').count()) === 1, "no invite: no link to the waitlist");
  check((await page.locator("#signin-form input:not([type=hidden])").count()) === 0, "sign-in asks for something");
  check(
    (await page.evaluate(() => window.__webauthnLog.length)) === 0,
    "the page opened a WebAuthn request before any button was pressed",
  );
  await shoot("signin-no-invite");

  // The invite link: an addressed one locks its address; an open link asks for one.
  await page.goto(`${base}/invite/${encodeURIComponent(inviteToken)}`);
  await page.waitForURL((url) => url.pathname === "/auth/login", { timeout: 10_000 });
  const field = page.locator("#signup-email");
  const readonly = await field.evaluate((input) => input.readOnly);
  const signupEmail = process.env.SIGNUP_EMAIL || `browser-${Date.now()}@example.com`;
  if (readonly) {
    check(Boolean(await field.inputValue()), "addressed invite: no address prefilled");
  } else {
    check((await field.inputValue()) === "", "open link: the address is already filled");
    await field.fill(signupEmail);
  }
  const optIn = page.locator('#signup-form [name="product_email_opt_in"]');
  check((await optIn.count()) === 1, "invite: no optional email checkbox");
  check(!(await optIn.isChecked()), "invite: the email checkbox starts checked");
  check((await page.locator('#signup-form [name="name"]').count()) === 0, "invite: a name field is offered");
  check((await page.locator("#signin-form").count()) === 0, "invite: the sign-in form is shown beside signup");
  await shoot("signup-invite");

  // The invite visitor's sign-in view, and the way back to signup.
  await page.locator('a[href="/auth/login?view=signin"]').click();
  await page.waitForURL((url) => url.search === "?view=signin", { timeout: 10_000 });
  check((await page.locator("#signup-form").count()) === 0, "invite sign-in view: a create-account form is shown");
  check((await page.locator("#signin-form").count()) === 1, "invite sign-in view: no sign-in form");
  await shoot("signin-invite");
  await page.locator('a[href="/auth/login"]').click();
  await page.waitForSelector("#signup-form", { timeout: 10_000 });
  if (!readonly) await page.locator("#signup-email").fill(signupEmail);
  await page.locator('#signup-form [name="product_email_opt_in"]').check();

  await page.locator('#signup-form button[type="submit"]').click();
  if (await signedIn("create account", "#signup-status")) {
    check(new URL(page.url()).pathname === "/app/", `create account landed on ${page.url()}, not /app/`);
    const member = await page.evaluate(() => !document.title.startsWith("Invite only"));
    check(member, "create account: /app/ says invite-only, so the invite was not redeemed");
  }

  await context.clearCookies();
  await page.goto(`${base}/auth/login`);
  await shoot("signin");
  await page.locator('#signin-form button[type="submit"]').click();
  await signedIn("sign in", "#signin-status");
  if (external.length) failures.push(`requests to another origin: ${external.join(", ")}`);
} finally {
  await browser.close();
}

assert.deepEqual(failures, [], failures.join("\n"));
console.log("ok");
