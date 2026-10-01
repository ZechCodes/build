// The signed-in passkey pages (add a passkey, verify a second factor), against a
// browser that runs one WebAuthn request at a time and refuses a second with "A
// request is already pending." Run by buildapp/test_auth_pages.py, so the Python
// gate covers it.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  bindPasskeyPage,
  createCeremonies,
  csrfPoster,
  describeError,
  describeRefusal,
} from "../buildapp/landing/passkey-ceremony.js";

const bytes = (text) => new TextEncoder().encode(text).buffer;
const later = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

function fakeCredential(kind) {
  const response =
    kind === "create"
      ? { attestationObject: bytes("att"), clientDataJSON: bytes("cdj"), getTransports: () => ["internal"] }
      : { authenticatorData: bytes("ad"), clientDataJSON: bytes("cdj"), signature: bytes("sig"), userHandle: null };
  return { id: `${kind}-id`, rawId: bytes(`${kind}-id`), type: "public-key", response };
}

class OneAtATimeCredentials {
  pending = false;
  log = [];
  outcome = "accept"; // or a DOMException name to reject with

  get(options) {
    return this.#request("get", options);
  }

  create(options) {
    return this.#request("create", options);
  }

  async #request(kind, options) {
    this.log.push(kind);
    if (this.pending) throw new DOMException("A request is already pending.", "InvalidStateError");
    assert.ok(options.publicKey.challenge instanceof Uint8Array, "the challenge is decoded");
    this.pending = true;
    await later(2);
    this.pending = false;
    if (this.outcome !== "accept") throw new DOMException("Cancelled.", this.outcome);
    return fakeCredential(kind);
  }
}

const AUTH_OPTIONS = { challenge: "Y2hhbGxlbmdl", allowCredentials: [{ id: "Y3JlZA", type: "public-key" }] };
const REGISTRATION_OPTIONS = {
  challenge: "Y2hhbGxlbmdl",
  user: { id: "dXNlcg", name: "a@example.com", displayName: "A" },
  excludeCredentials: [],
};

function fakeServer(overrides = {}) {
  const calls = [];
  const answers = {
    options: (fields) => ({
      ok: true,
      status: 200,
      payload: { options: "display_name" in fields ? REGISTRATION_OPTIONS : AUTH_OPTIONS },
    }),
    complete: () => ({ ok: true, status: 200, payload: { ok: true, redirect: "/app/" } }),
    ...overrides,
  };
  async function post(path, fields) {
    calls.push({ path, fields });
    await later();
    return answers[path](fields);
  }
  return { calls, post };
}

test("adding a passkey posts options then complete, with its name both times", async () => {
  const server = fakeServer();
  const credentials = new OneAtATimeCredentials();
  const outcome = await createCeremonies({ credentials, post: server.post }).register("Laptop");
  assert.deepEqual(outcome, { added: true });
  assert.deepEqual(credentials.log, ["create"]);
  assert.deepEqual(server.calls.map((call) => call.path), ["options", "complete"]);
  assert.equal(server.calls[0].fields.display_name, "Laptop");
  assert.equal(server.calls[1].fields.display_name, "Laptop");
  assert.equal(JSON.parse(server.calls[1].fields.credential).response.transports[0], "internal");
});

test("verifying posts options then complete and follows the server's redirect", async () => {
  const server = fakeServer();
  const credentials = new OneAtATimeCredentials();
  const outcome = await createCeremonies({ credentials, post: server.post }).verify();
  assert.deepEqual(outcome, { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get"]);
  assert.deepEqual(server.calls.map((call) => call.path), ["options", "complete"]);
  assert.equal(JSON.parse(server.calls[1].fields.credential).id, "get-id");
});

for (const kind of ["register", "verify"]) {
  test(`a second ${kind} press while one runs is ignored and opens no second request`, async () => {
    const server = fakeServer();
    const credentials = new OneAtATimeCredentials();
    const ceremonies = createCeremonies({ credentials, post: server.post });
    const run = () => (kind === "register" ? ceremonies.register("") : ceremonies.verify());
    const first = run();
    assert.equal(ceremonies.isBusy(), true);
    assert.deepEqual(await run(), { ignored: true });
    const outcome = await first;
    assert.equal(outcome.error, undefined);
    assert.equal(credentials.log.length, 1);
    assert.equal(ceremonies.isBusy(), false);
  });
}

test("after a cancelled prompt the next attempt works", async () => {
  const server = fakeServer();
  const credentials = new OneAtATimeCredentials();
  const ceremonies = createCeremonies({ credentials, post: server.post });
  credentials.outcome = "NotAllowedError";
  assert.match((await ceremonies.verify()).error, /closed or timed out/);
  credentials.outcome = "accept";
  assert.deepEqual(await ceremonies.verify(), { redirect: "/app/" });
});

test("a refusal reads as a sentence a person can act on", async () => {
  const server = fakeServer({ options: () => ({ ok: false, status: 401, payload: { error: "pending_auth_missing" } }) });
  const outcome = await createCeremonies({ credentials: new OneAtATimeCredentials(), post: server.post }).verify();
  assert.equal(outcome.error, "This sign-in has expired. Sign in again.");
  assert.equal(describeRefusal("register", { status: 400, payload: { error: "invalid_credential" } }), "That passkey couldn't be added. Try again.");
  assert.equal(describeRefusal("verify", { status: 429, payload: {} }), "Too many attempts. Wait a minute and try again.");
  assert.equal(describeRefusal("verify", { status: 400, payload: { error: "Something internal" } }), "Checking your passkey didn't work. Try again.");
  assert.equal(describeError("register", { name: "InvalidStateError" }), "This device already has a passkey for your account.");
});

function fakeDocument(token) {
  const input = { value: token };
  return { input, querySelectorAll: () => [input] };
}

function fakeFetch(answers) {
  const calls = [];
  async function fetchImpl(url, init = {}) {
    calls.push({ url, method: init.method || "GET", csrf: init.body?.get("_csrf") });
    const answer = answers[`${init.method || "GET"} ${url}`];
    return { ok: answer.status < 400, status: answer.status, json: async () => answer.json, text: async () => answer.text };
  }
  return { calls, fetchImpl };
}

test("a fresh token in the answer goes into the page's forms", async () => {
  const document = fakeDocument("old");
  const { calls, fetchImpl } = fakeFetch({ "POST /auth/passkeys/options": { status: 200, json: { csrf_token: "new" } } });
  const post = csrfPoster({ document, fetchImpl, base: "/auth/passkeys/", pageUrl: "/auth/passkeys" });
  await post("options", { display_name: "" });
  assert.equal(calls[0].csrf, "old");
  assert.equal(document.input.value, "new");
  assert.equal(calls.length, 1);
});

test("a refusal without a token re-reads it from the page, so the next attempt is not stale", async () => {
  const document = fakeDocument("old");
  const { calls, fetchImpl } = fakeFetch({
    "POST /auth/passkeys/complete": { status: 400, json: { error: "invalid_credential" } },
    "GET /auth/passkeys": { status: 200, text: '<input type="hidden" name="_csrf" value="rotated">' },
  });
  const post = csrfPoster({ document, fetchImpl, base: "/auth/passkeys/", pageUrl: "/auth/passkeys" });
  const response = await post("complete", {});
  assert.equal(response.ok, false);
  assert.deepEqual(calls.map((call) => `${call.method} ${call.url}`), ["POST /auth/passkeys/complete", "GET /auth/passkeys"]);
  assert.equal(document.input.value, "rotated");
});

test("a page with no passkey form binds nothing", () => {
  const document = { getElementById: () => null };
  assert.equal(bindPasskeyPage(document, {}), null);
});

test("a browser without passkeys says so and disables the button", () => {
  const button = { disabled: false };
  const form = { querySelector: () => button, dataset: { factorKey: "passkey" } };
  const status = { textContent: "", dataset: {} };
  const elements = { "passkey-verify-form": form, "verify-status": status };
  const document = { getElementById: (id) => elements[id] || null };
  assert.equal(bindPasskeyPage(document, { navigator: {} }), null);
  assert.equal(button.disabled, true);
  assert.match(status.textContent, /can't use passkeys/);
  assert.equal(status.dataset.tone, "error");
});

test("a successful answer without a token leaves the page alone: no hidden GET", async () => {
  // On verify, Skrift has already cleared the pending sign-in; a GET of the page now
  // would queue "Your verification session is no longer available" for the next page.
  const document = fakeDocument("old");
  const { calls, fetchImpl } = fakeFetch({
    "POST /auth/verify/passkey/complete": { status: 200, json: { ok: true, redirect: "/app/" } },
  });
  const post = csrfPoster({ document, fetchImpl, base: "/auth/verify/passkey/", pageUrl: "/auth/verify/passkey" });
  const response = await post("complete", {});
  assert.equal(response.ok, true);
  assert.deepEqual(calls.map((call) => `${call.method} ${call.url}`), ["POST /auth/verify/passkey/complete"]);
});

test("a network failure says Build couldn't be reached", async () => {
  const document = fakeDocument("t");
  const fetchImpl = async () => {
    throw new TypeError("Failed to fetch");
  };
  const post = csrfPoster({ document, fetchImpl, base: "/auth/passkeys/", pageUrl: "/auth/passkeys" });
  const outcome = await createCeremonies({ credentials: new OneAtATimeCredentials(), post }).register("");
  assert.equal(outcome.error, "Couldn't reach Build. Check your connection and try again.");
});

test("a malformed answer is not mistaken for a network failure", async () => {
  const server = fakeServer({ options: () => ({ ok: true, status: 200, payload: {} }) });
  const credentials = new OneAtATimeCredentials();
  const ceremonies = createCeremonies({ credentials, post: server.post });
  assert.equal((await ceremonies.register("")).error, "Adding your passkey didn't work. Try again.");
  assert.equal((await ceremonies.verify()).error, "Checking your passkey didn't work. Try again.");
  assert.deepEqual(credentials.log, []);
});
