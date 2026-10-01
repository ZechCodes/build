// Build's sign-in and create-account page (templates/auth/passkey_login.html).
// Same endpoints and payloads as Skrift's stock passkey page. Sign-in is one button:
// a modal get() with no allowCredentials, so the browser lists the passkeys it holds
// for this site and nobody types anything. The page opens no WebAuthn request on its
// own (no conditional autofill: it needs a visible text field to hang from, and a
// browser runs one request at a time, so Safari answered a press while it was open
// with "A request is already pending."). Account creation posts the invite's address,
// which the page shows read-only; the server takes no other (#314).

export function base64urlToBuffer(value) {
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  const base64 = (value + padding).replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
}

export function bufferToBase64url(buffer) {
  let binary = "";
  new Uint8Array(buffer).forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function withCredentialIds(list) {
  return (list || []).map((credential) => ({ ...credential, id: base64urlToBuffer(credential.id) }));
}

function authenticationOptions(options) {
  return {
    ...options,
    challenge: base64urlToBuffer(options.challenge),
    allowCredentials: withCredentialIds(options.allowCredentials),
  };
}

function registrationOptions(options) {
  return {
    ...options,
    challenge: base64urlToBuffer(options.challenge),
    user: { ...options.user, id: base64urlToBuffer(options.user.id) },
    excludeCredentials: withCredentialIds(options.excludeCredentials),
  };
}

function serializeAssertion(credential) {
  const response = credential.response;
  return {
    id: credential.id,
    rawId: bufferToBase64url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: bufferToBase64url(response.clientDataJSON),
      authenticatorData: bufferToBase64url(response.authenticatorData),
      signature: bufferToBase64url(response.signature),
      userHandle: response.userHandle ? bufferToBase64url(response.userHandle) : null,
    },
  };
}

function serializeRegistration(credential) {
  const response = credential.response;
  return {
    id: credential.id,
    rawId: bufferToBase64url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: bufferToBase64url(response.attestationObject),
      clientDataJSON: bufferToBase64url(response.clientDataJSON),
      transports: response.getTransports ? response.getTransports() : [],
    },
  };
}

// --- what a person reads when something fails --------------------------------

const SERVER_MESSAGES = {
  // The refusal carried a fresh token and the poster has already put it in the
  // forms, so pressing the button again is enough.
  invalid_csrf: "That attempt didn't go through. Try again.",
  invalid_credential: "That passkey isn't registered with Build. Choose a different passkey.",
  credential_id_required: "That passkey isn't registered with Build. Choose a different passkey.",
  invite_required: "This page no longer holds a usable invite. Open the link in your invite email again.",
  registration_state_missing: "That took too long. Try again.",
};

const SIGNUP_REFUSED =
  "We couldn't create an account with that email. If you already have one, sign in above instead.";

const FALLBACK = {
  signin: "Sign-in didn't work. Try again.",
  signup: "Creating your account didn't work. Try again.",
};

/** The sentence for a refusal from the server: `{ status, payload }`. */
export function describeRefusal(kind, { status, payload }) {
  if (status === 429) return "Too many attempts. Wait a minute and try again.";
  if (status === 503) return "Passkeys aren't available right now. Try again in a few minutes.";
  const code = payload && payload.error;
  if (kind === "signup" && code === "invalid_request") return SIGNUP_REFUSED;
  return SERVER_MESSAGES[code] || FALLBACK[kind];
}

/** The sentence for an error the browser threw: WebAuthn's DOMExceptions and fetch's TypeError. */
export function describeError(kind, error) {
  switch (error && error.name) {
    case "NotAllowedError":
    case "AbortError":
      return "The passkey prompt was closed or timed out. Try again when you're ready.";
    case "InvalidStateError":
      return kind === "signup"
        ? "This device already has a passkey for that email. Sign in above instead."
        : "Your browser is still finishing another passkey request. Wait a moment and try again.";
    case "SecurityError":
      return "Passkeys only work on getbuild.ing. Open the page there and try again.";
    case "NotSupportedError":
      return "This device can't use a passkey here. Try another browser or device.";
    case "TypeError":
      return "Couldn't reach Build. Check your connection and try again.";
    default:
      return FALLBACK[kind];
  }
}

// --- the ceremonies -----------------------------------------------------------

class Refused extends Error {
  constructor(response) {
    super("refused");
    this.response = response;
  }
}

/**
 * The page's WebAuthn ceremonies, free of the DOM so they can be tested with a fake
 * browser.
 *
 * - `credentials`: navigator.credentials.
 * - `post(path, fields)`: POSTs to /auth/<method>/<path> with the CSRF token,
 *   resolves `{ ok, status, payload }`; rejects only when the network does.
 *
 * Every outcome is `{ redirect }`, `{ error }` (a sentence), or `{ ignored: true }`
 * for a press that arrived while another ceremony was running.
 */
export function createPasskeyFlow({ credentials, post }) {
  let busy = false;
  let leaving = false;

  async function exclusive(kind, ceremony) {
    if (busy || leaving) return { ignored: true };
    busy = true;
    try {
      const outcome = await ceremony();
      leaving = true;
      return outcome;
    } catch (error) {
      return { error: failure(kind, error) };
    } finally {
      busy = false;
    }
  }

  function signIn() {
    return exclusive("signin", async () => {
      const response = await post("options", {});
      if (!response.ok) throw new Refused(response);
      const credential = await credentials.get({ publicKey: authenticationOptions(response.payload.options) });
      const completion = await post("complete", { credential: JSON.stringify(serializeAssertion(credential)) });
      if (!completion.ok) throw new Refused(completion);
      return { redirect: completion.payload.redirect || "/" };
    });
  }

  function signUp(fields) {
    return exclusive("signup", async () => {
      const response = await post("register/options", fields);
      if (!response.ok) throw new Refused(response);
      const credential = await credentials.create({ publicKey: registrationOptions(response.payload.options) });
      const completion = await post("register/complete", {
        credential: JSON.stringify(serializeRegistration(credential)),
      });
      if (!completion.ok) throw new Refused(completion);
      return { redirect: completion.payload.redirect || "/" };
    });
  }

  return { signIn, signUp, isBusy: () => busy };
}

function failure(kind, error) {
  return error instanceof Refused ? describeRefusal(kind, error.response) : describeError(kind, error);
}

// --- the page -------------------------------------------------------------------

function csrfPoster(document, fetchImpl, methodKey) {
  const inputs = () => document.querySelectorAll('input[name="_csrf"]');
  return async function post(path, fields) {
    const body = new FormData();
    body.append("_csrf", inputs()[0]?.value || "");
    for (const [name, value] of Object.entries(fields)) body.append(name, value);
    const response = await fetchImpl(`/auth/${encodeURIComponent(methodKey)}/${path}`, {
      method: "POST",
      body,
      credentials: "same-origin",
    });
    const payload = await response.json().catch(() => ({}));
    // The server rotates the token on every CSRF check, pass or fail; every form
    // on the page shares it.
    if (typeof payload.csrf_token === "string" && payload.csrf_token) {
      inputs().forEach((input) => {
        input.value = payload.csrf_token;
      });
    }
    return { ok: response.ok, status: response.status, payload };
  };
}

function bindForm({ form, status, busyText, doneText, run, page }) {
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (page.busy) return;
    page.setBusy(form, true);
    page.say(status, busyText);
    const outcome = await run();
    if (outcome.ignored) return;
    if (outcome.redirect) {
      page.say(status, doneText);
      page.go(outcome.redirect);
      return;
    }
    page.setBusy(form, false);
    page.say(status, outcome.error, "error");
  });
}

/** The page's forms with their status lines: sign-in always, create-account only
 *  when the server found an invite in this session. */
function pageForms(document) {
  return [
    ["signin", "signin-form", "signin-status"],
    ["signup", "signup-form", "signup-status"],
  ]
    .map(([kind, formId, statusId]) => ({
      kind,
      form: document.getElementById(formId),
      status: document.getElementById(statusId),
    }))
    .filter(({ form }) => form);
}

const CEREMONIES = {
  signin: {
    busyText: "Waiting for your passkey…",
    doneText: "Signed in. Opening Build…",
    run: (flow) => flow.signIn(),
  },
  signup: {
    busyText: "Creating your passkey…",
    doneText: "Account created. Opening Build…",
    run: (flow, form) => flow.signUp({ email: String(new FormData(form).get("email") || "") }),
  },
};

export function bindSigninPage(document, window) {
  const root = document.querySelector("[data-passkey-method]");
  const forms = pageForms(document);
  // No forms when the server says passkeys are unavailable: nothing to bind.
  if (!root || !forms.length) return null;

  if (!window.PublicKeyCredential || !window.navigator.credentials) {
    for (const { form, status } of forms) {
      form.querySelector('button[type="submit"]').disabled = true;
      status.textContent = "This browser can't use passkeys. Open Build in an up-to-date browser on your phone or computer.";
      status.dataset.tone = "error";
    }
    return null;
  }

  const flow = createPasskeyFlow({
    credentials: window.navigator.credentials,
    post: csrfPoster(document, window.fetch.bind(window), root.dataset.passkeyMethod),
  });

  const buttons = forms.map(({ form }) => form.querySelector('button[type="submit"]'));
  const page = {
    busy: false,
    setBusy(form, busy) {
      this.busy = busy;
      buttons.forEach((button) => {
        button.disabled = busy;
      });
      if (busy) form.setAttribute("aria-busy", "true");
      else form.removeAttribute("aria-busy");
    },
    say(status, text, tone = "") {
      for (const other of forms) {
        if (other.status !== status) other.status.textContent = "";
      }
      status.textContent = text;
      status.dataset.tone = tone;
    },
    go: (url) => window.location.assign(url),
  };

  for (const { kind, form, status } of forms) {
    const { busyText, doneText, run } = CEREMONIES[kind];
    bindForm({ form, status, busyText, doneText, run: () => run(flow, form), page });
  }
  return flow;
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  bindSigninPage(document, window);
}
