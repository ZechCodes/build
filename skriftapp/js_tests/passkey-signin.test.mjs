// The sign-in page's WebAuthn ordering, against a browser that behaves like Safari:
// one request at a time, a second one refused with "A request is already pending.",
// and an aborted request let go of only when its rejection arrives, a task later.
// Run by buildapp/test_passkey_signin_page.py, so the Python gate covers it.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  createPasskeyFlow,
  describeError,
} from "../buildapp/landing/passkey-signin.js";

const PENDING = "A request is already pending.";
const bytes = (text) => new TextEncoder().encode(text).buffer;
const later = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

function fakeCredential(kind) {
  const response =
    kind === "create"
      ? { attestationObject: bytes("att"), clientDataJSON: bytes("cdj"), getTransports: () => ["internal"] }
      : { authenticatorData: bytes("ad"), clientDataJSON: bytes("cdj"), signature: bytes("sig"), userHandle: bytes("uh") };
  return { id: `${kind}-id`, rawId: bytes(`${kind}-id`), type: "public-key", response };
}

class SafariCredentials {
  pending = null;
  log = [];
  // What the next modal request does: "accept", or a DOMException name to reject with.
  modal = "accept";

  get(options) {
    return this.#request("get", options);
  }

  create(options) {
    return this.#request("create", options);
  }

  // The person picks a saved passkey from the email field's autofill.
  pickAutofill() {
    const entry = this.pending;
    assert.equal(entry?.mediation, "conditional", "no autofill request is open");
    this.pending = null;
    entry.resolve(fakeCredential("get"));
  }

  #request(kind, options) {
    const mediation = options.mediation || "modal";
    this.log.push(`${kind}:${mediation}`);
    if (this.pending) {
      return Promise.reject(new DOMException(PENDING, "InvalidStateError"));
    }
    return new Promise((resolve, reject) => {
      const entry = { mediation, resolve };
      this.pending = entry;
      options.signal?.addEventListener("abort", () =>
        setTimeout(() => {
          if (this.pending === entry) this.pending = null;
          reject(new DOMException("The operation was aborted.", "AbortError"));
        }, 5),
      );
      if (mediation === "modal") {
        setTimeout(() => {
          if (this.pending !== entry) return;
          this.pending = null;
          if (this.modal === "accept") resolve(fakeCredential(kind));
          else reject(new DOMException("Cancelled.", this.modal));
        }, 1);
      }
    });
  }
}

const AUTH_OPTIONS = { challenge: "Y2hhbGxlbmdl", allowCredentials: [] };
const REGISTRATION_OPTIONS = {
  challenge: "Y2hhbGxlbmdl",
  user: { id: "dXNlcg", name: "a@example.com", displayName: "A" },
  excludeCredentials: [],
};

function fakeServer(overrides = {}) {
  const calls = [];
  const answers = {
    options: () => ({ ok: true, status: 200, payload: { options: AUTH_OPTIONS } }),
    complete: () => ({ ok: true, status: 200, payload: { redirect: "/app/" } }),
    "register/options": () => ({ ok: true, status: 200, payload: { options: REGISTRATION_OPTIONS } }),
    "register/complete": () => ({ ok: true, status: 201, payload: { redirect: "/app/" } }),
    ...overrides,
  };
  return {
    calls,
    post: async (path, fields) => {
      calls.push(path);
      return answers[path](fields);
    },
  };
}

function page({ server = fakeServer(), credentials = new SafariCredentials() } = {}) {
  const flow = createPasskeyFlow({
    credentials,
    post: server.post,
    conditionalAvailable: async () => true,
  });
  return { flow, server, credentials };
}

async function autofillOpen(credentials) {
  for (let i = 0; i < 50 && credentials.pending?.mediation !== "conditional"; i += 1) await later();
  assert.equal(credentials.pending?.mediation, "conditional", "autofill never opened");
}

test("the stock page's ordering is the bug: create() while autofill is open is refused", async () => {
  const credentials = new SafariCredentials();
  credentials.get({ publicKey: {}, mediation: "conditional", signal: new AbortController().signal });
  await assert.rejects(credentials.create({ publicKey: {} }), { name: "InvalidStateError", message: PENDING });
});

test("creating an account while autofill is open aborts it first and succeeds", async () => {
  const { flow, server, credentials } = page();
  flow.startAutofill();
  await autofillOpen(credentials);

  const outcome = await flow.signUp({ email: "a@example.com", name: "A" });

  assert.deepEqual(outcome, { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get:conditional", "create:modal"]);
  assert.deepEqual(server.calls, ["options", "register/options", "register/complete"]);
});

test("creating an account while autofill is still fetching its options never opens autofill", async () => {
  let releaseOptions;
  const server = fakeServer({
    options: () =>
      new Promise((resolve) => {
        releaseOptions = () => resolve({ ok: true, status: 200, payload: { options: AUTH_OPTIONS } });
      }),
  });
  const { flow, credentials } = page({ server });
  flow.startAutofill();
  await later();

  const signUp = flow.signUp({ email: "a@example.com", name: "" });
  releaseOptions();

  assert.deepEqual(await signUp, { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["create:modal"]);
});

test("signing in with the button while autofill is open aborts it first and succeeds", async () => {
  const { flow, credentials } = page();
  flow.startAutofill();
  await autofillOpen(credentials);

  assert.deepEqual(await flow.signIn(), { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get:conditional", "get:modal"]);
});

test("a second press while a ceremony runs is ignored", async () => {
  const { flow, server } = page();
  const first = flow.signUp({ email: "a@example.com", name: "" });
  const second = flow.signUp({ email: "a@example.com", name: "" });
  const third = flow.signIn();

  assert.deepEqual(await second, { ignored: true });
  assert.deepEqual(await third, { ignored: true });
  assert.deepEqual(await first, { redirect: "/app/" });
  assert.equal(server.calls.filter((path) => path === "register/options").length, 1);
});

test("after a cancelled attempt autofill comes back and the next attempt works", async () => {
  const { flow, credentials } = page();
  flow.startAutofill();
  await autofillOpen(credentials);
  credentials.modal = "NotAllowedError";

  const cancelled = await flow.signUp({ email: "a@example.com", name: "" });
  assert.equal(cancelled.error, describeError("signup", { name: "NotAllowedError" }));

  flow.startAutofill();
  await autofillOpen(credentials);
  credentials.modal = "accept";
  assert.deepEqual(await flow.signUp({ email: "a@example.com", name: "" }), { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get:conditional", "create:modal", "get:conditional", "create:modal"]);
});

test("picking a passkey from autofill signs in, and a press after that is ignored", async () => {
  const { flow, credentials } = page();
  const autofill = flow.startAutofill();
  await autofillOpen(credentials);

  credentials.pickAutofill();

  assert.deepEqual(await autofill, { redirect: "/app/" });
  assert.deepEqual(await flow.signUp({ email: "a@example.com", name: "" }), { ignored: true });
});

test("a refusal reads as a sentence a person can act on", async () => {
  const server = fakeServer({
    "register/options": () => ({ ok: false, status: 400, payload: { error: "invalid_request" } }),
  });
  const { flow } = page({ server });

  const { error } = await flow.signUp({ email: "taken@example.com", name: "" });

  assert.match(error, /If you already have one, sign in/);
});

test("a passkey this device already holds for that email says to sign in", () => {
  assert.match(describeError("signup", { name: "InvalidStateError" }), /already has a passkey/);
  assert.match(describeError("signin", new TypeError("Failed to fetch")), /Couldn't reach Build/);
});
