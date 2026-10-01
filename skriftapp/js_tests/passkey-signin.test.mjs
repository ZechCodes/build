// The sign-in page's WebAuthn ceremonies, against a browser that behaves like Safari:
// one request at a time, a second one refused with "A request is already pending.".
// The page opens no request of its own, so a press never meets one already open.
// Run by buildapp/test_passkey_signin_page.py, so the Python gate covers it.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  bindSigninPage,
  createPasskeyFlow,
  describeError,
  describeRefusal,
  withArrivalFragment,
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

  #request(kind, options) {
    const mediation = options.mediation || "modal";
    this.log.push(`${kind}:${mediation}`);
    if (this.pending) {
      return Promise.reject(new DOMException(PENDING, "InvalidStateError"));
    }
    return new Promise((resolve, reject) => {
      const entry = { mediation, resolve };
      this.pending = entry;
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
  const flow = createPasskeyFlow({ credentials, post: server.post });
  return { flow, server, credentials };
}

/** Just enough DOM for bindSigninPage: elements by id, forms that remember their
 *  submit listener, and a FormData that reads a form's fields. */
function fakeDocument({ signin = true, signup = true } = {}) {
  const element = (extra = {}) => ({
    dataset: {},
    textContent: "",
    attributes: {},
    setAttribute(name, value) {
      this.attributes[name] = value;
    },
    removeAttribute(name) {
      delete this.attributes[name];
    },
    ...extra,
  });
  const form = (fields) => {
    const button = element({ disabled: false });
    return element({
      fields,
      button,
      querySelector: () => button,
      addEventListener(type, listener) {
        this.submit = () => listener({ preventDefault() {} });
      },
    });
  };
  const elements = {
    ...(signin ? { "signin-form": form({}), "signin-status": element() } : {}),
    ...(signup ? { "signup-form": form({ email: "invitee@example.com" }), "signup-status": element() } : {}),
  };
  const csrf = { value: "token" };
  return {
    elements,
    querySelector: (selector) => (selector === "[data-passkey-method]" ? { dataset: { passkeyMethod: "passkey" } } : null),
    querySelectorAll: () => [csrf],
    getElementById: (id) => elements[id] || null,
  };
}

function fakeWindow(credentials, posted) {
  return {
    PublicKeyCredential: function PublicKeyCredential() {},
    navigator: { credentials },
    location: { assign() {} },
    fetch: async (url, { body }) => {
      posted.push([url, Object.fromEntries(body.entries())]);
      const path = url.replace("/auth/passkey/", "");
      const payload = fakeServer().post(path).then((answer) => answer.payload);
      return { ok: true, status: 200, json: () => payload };
    },
  };
}

class FakeFormData {
  constructor(form) {
    this.form = form;
    this.values = [];
  }

  append(name, value) {
    this.values.push([name, value]);
  }

  entries() {
    return this.values[Symbol.iterator]();
  }

  get(name) {
    return this.form?.fields?.[name] ?? null;
  }
}

test("signing in is one modal request that lists the passkeys the browser holds", async () => {
  const { flow, server, credentials } = page();

  assert.deepEqual(await flow.signIn(), { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get:modal"]);
  assert.deepEqual(server.calls, ["options", "complete"]);
});

test("creating an account is one modal create() for the address it was given", async () => {
  const sent = [];
  const server = fakeServer();
  const { flow, credentials } = page({
    server: { calls: server.calls, post: (path, fields) => (sent.push([path, fields]), server.post(path, fields)) },
  });

  assert.deepEqual(await flow.signUp({ email: "invitee@example.com" }), { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["create:modal"]);
  assert.deepEqual(sent[0], ["register/options", { email: "invitee@example.com" }]);
});

test("the page opens no WebAuthn request until a button is pressed", async () => {
  globalThis.FormData = FakeFormData;
  const credentials = new SafariCredentials();
  const posted = [];
  const document = fakeDocument();

  assert.ok(bindSigninPage(document, fakeWindow(credentials, posted)));
  await later(10);

  assert.deepEqual(credentials.log, []);
  assert.deepEqual(posted, []);
});

test("the create-account button posts the invite's address and nothing else", async () => {
  globalThis.FormData = FakeFormData;
  const posted = [];
  const document = fakeDocument();
  bindSigninPage(document, fakeWindow(new SafariCredentials(), posted));

  document.elements["signup-form"].submit();
  for (let i = 0; i < 50 && posted.length < 2; i += 1) await later();

  const [url, fields] = posted[0];
  assert.equal(url, "/auth/passkey/register/options");
  assert.deepEqual(fields, { _csrf: "token", email: "invitee@example.com" });
});

test("a page with no invite binds its sign-in button alone", async () => {
  globalThis.FormData = FakeFormData;
  const credentials = new SafariCredentials();
  const document = fakeDocument({ signup: false });
  assert.ok(bindSigninPage(document, fakeWindow(credentials, [])));

  document.elements["signin-form"].submit();
  for (let i = 0; i < 50 && !credentials.log.length; i += 1) await later();

  assert.deepEqual(credentials.log, ["get:modal"]);
});

test("an invite visitor's page binds its create-account button alone", async () => {
  globalThis.FormData = FakeFormData;
  const credentials = new SafariCredentials();
  const posted = [];
  const document = fakeDocument({ signin: false });
  assert.ok(bindSigninPage(document, fakeWindow(credentials, posted)));

  document.elements["signup-form"].submit();
  for (let i = 0; i < 50 && !credentials.log.length; i += 1) await later();

  assert.deepEqual(credentials.log, ["create:modal"]);
  assert.equal(posted[0][0], "/auth/passkey/register/options");
});

test("no message points at a sign-in button the page no longer shows beside it", () => {
  const sentences = [
    describeRefusal("signup", { status: 400, payload: { error: "invalid_request" } }),
    describeError("signup", { name: "InvalidStateError" }),
  ];
  for (const sentence of sentences) assert.doesNotMatch(sentence, /above|below/);
});

test("a second press while a ceremony runs is ignored", async () => {
  const { flow, server } = page();
  const first = flow.signUp({ email: "a@example.com" });
  const second = flow.signUp({ email: "a@example.com" });
  const third = flow.signIn();

  assert.deepEqual(await second, { ignored: true });
  assert.deepEqual(await third, { ignored: true });
  assert.deepEqual(await first, { redirect: "/app/" });
  assert.equal(server.calls.filter((path) => path === "register/options").length, 1);
});

test("after a cancelled attempt the next attempt works", async () => {
  const { flow, credentials } = page();
  credentials.modal = "NotAllowedError";

  const cancelled = await flow.signIn();
  assert.equal(cancelled.error, describeError("signin", { name: "NotAllowedError" }));

  credentials.modal = "accept";
  assert.deepEqual(await flow.signIn(), { redirect: "/app/" });
  assert.deepEqual(credentials.log, ["get:modal", "get:modal"]);
});

test("a passkey Build does not know says so", async () => {
  const server = fakeServer({ complete: () => ({ ok: false, status: 400, payload: { error: "invalid_credential" } }) });
  const { error } = await page({ server }).flow.signIn();

  assert.match(error, /isn't registered with Build/);
});

test("a refusal reads as a sentence a person can act on", async () => {
  const server = fakeServer({
    "register/options": () => ({ ok: false, status: 400, payload: { error: "invalid_request" } }),
  });
  const { flow } = page({ server });

  const { error } = await flow.signUp({ email: "taken@example.com" });

  assert.match(error, /If you already have one, sign in/);
});

test("an invite the server no longer accepts says to open the link again", () => {
  assert.match(describeRefusal("signup", { status: 403, payload: { error: "invite_required" } }), /invite email again/);
});

test("a passkey this device already holds for that email says to sign in", () => {
  assert.match(describeError("signup", { name: "InvalidStateError" }), /already has a passkey/);
  assert.match(describeError("signin", new TypeError("Failed to fetch")), /Couldn't reach Build/);
});

test("a page with no forms (passkeys unavailable) binds nothing and throws nothing", () => {
  const root = { dataset: { passkeyMethod: "passkey" } };
  const document = {
    querySelector: (selector) => (selector === "[data-passkey-method]" ? root : null),
    getElementById: () => null,
  };
  const window = { PublicKeyCredential: function PublicKeyCredential() {}, navigator: { credentials: {} } };

  assert.equal(bindSigninPage(document, window), null);
  assert.equal(bindSigninPage(document, { navigator: {} }), null);
});

test("a stale security token says to try again, which works: the fresh one is already in the form", () => {
  assert.match(describeRefusal("signin", { status: 400, payload: { error: "invalid_csrf" } }), /Try again/);
  assert.doesNotMatch(describeRefusal("signup", { status: 400, payload: { error: "invalid_csrf" } }), /[Rr]eload/);
});

// /app/ sends a signed-out visitor here with a 302, and the browser keeps the
// fragment across it; the server never sees one, so the page puts it back on
// where it goes next. That is how the bridge's approve link (#319) still opens
// the approve screen once the visitor has signed in.
const ARRIVED = { origin: "https://getbuild.ing", href: "https://getbuild.ing/auth/login?next=/app/#/pair/ZSAC-ABU6", hash: "#/pair/ZSAC-ABU6" };

test("a signed-in visitor goes on with the fragment they arrived with", () => {
  assert.equal(withArrivalFragment("/app/", ARRIVED), "https://getbuild.ing/app/#/pair/ZSAC-ABU6");
  assert.equal(withArrivalFragment("https://getbuild.ing/app/?x=1", ARRIVED), "https://getbuild.ing/app/?x=1#/pair/ZSAC-ABU6");
});

test("a redirect with its own fragment, or to another origin, is left as the server said", () => {
  assert.equal(withArrivalFragment("/app/#/inbox", ARRIVED), "/app/#/inbox");
  assert.equal(withArrivalFragment("https://elsewhere.example/app/", ARRIVED), "https://elsewhere.example/app/");
  assert.equal(withArrivalFragment("//elsewhere.example/app/", ARRIVED), "//elsewhere.example/app/");
});

// A same-origin redirect whose path starts with `//` must not come back as a
// protocol-relative address that leaves the site (#319 review).
test("a same-origin path that starts with // stays on this site", () => {
  const out = withArrivalFragment("https://getbuild.ing//evil.example", ARRIVED);
  assert.equal(new URL(out, ARRIVED.href).origin, "https://getbuild.ing");
  assert.equal(out, "https://getbuild.ing//evil.example#/pair/ZSAC-ABU6");
});

test("a visitor who arrived with no fragment goes exactly where the server said", () => {
  assert.equal(withArrivalFragment("/app/", { ...ARRIVED, hash: "" }), "/app/");
});

test("the page goes on with the fragment once the passkey is accepted", async () => {
  globalThis.FormData = FakeFormData;
  const document = fakeDocument({ signup: false });
  const went = [];
  const window = fakeWindow(new SafariCredentials(), []);
  window.location = { ...ARRIVED, assign: (url) => went.push(url) };
  bindSigninPage(document, window);
  document.elements["signin-form"].submit();
  await later(20);
  assert.equal(went.length, 1);
  assert.match(went[0], /#\/pair\/ZSAC-ABU6$/);
});
