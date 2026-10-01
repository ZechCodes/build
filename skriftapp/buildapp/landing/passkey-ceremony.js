// The signed-in passkey pages: adding one at /auth/passkeys (templates/auth/passkeys.html)
// and the second-factor check at /auth/verify/<key> (templates/auth/verify_passkey.html).
// Same endpoints and payloads as Skrift's stock pages, and the sign-in page's rule
// (passkey-signin.js): a browser runs one WebAuthn request at a time, so a press while
// a ceremony runs is ignored rather than starting a second request. Neither page keeps
// an autofill request open, so there is nothing to abort first.

import { base64urlToBuffer, bufferToBase64url } from "./passkey-signin.js";

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
  invalid_csrf: "That attempt didn't go through. Try again.",
  unauthorized: "You're signed out. Sign in again to manage your passkeys.",
  pending_auth_missing: "This sign-in has expired. Sign in again.",
  invalid_request: "This sign-in can't be checked with a passkey. Sign in again.",
};

const WRONG_PASSKEY = {
  register: "That passkey couldn't be added. Try again.",
  verify: "That passkey isn't one of yours. Choose a different passkey.",
};

const FALLBACK = {
  register: "Adding your passkey didn't work. Try again.",
  verify: "Checking your passkey didn't work. Try again.",
};

/** The sentence for a refusal from the server: `{ status, payload }`. */
export function describeRefusal(kind, { status, payload }) {
  if (status === 429) return "Too many attempts. Wait a minute and try again.";
  if (status === 503) return "Passkeys aren't available right now. Try again in a few minutes.";
  const code = payload && payload.error;
  if (["invalid_credential", "credential_id_required", "credential_required"].includes(code)) {
    return WRONG_PASSKEY[kind];
  }
  return SERVER_MESSAGES[code] || FALLBACK[kind];
}

/** The sentence for an error the browser threw: WebAuthn's DOMExceptions and fetch's TypeError. */
export function describeError(kind, error) {
  switch (error && error.name) {
    case "NotAllowedError":
    case "AbortError":
      return "The passkey prompt was closed or timed out. Try again when you're ready.";
    case "InvalidStateError":
      return kind === "register"
        ? "This device already has a passkey for your account."
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
 * The pages' WebAuthn state machine, free of the DOM so it can be tested with a fake
 * browser.
 *
 * - `credentials`: navigator.credentials.
 * - `post(path, fields)`: POSTs to the page's endpoint base (/auth/passkeys/ or
 *   /auth/verify/<key>/) with the CSRF token, resolves `{ ok, status, payload }`;
 *   rejects only when the network does.
 *
 * Every outcome is `{ added: true }`, `{ redirect }`, `{ error }` (a sentence), or
 * `{ ignored: true }` for a press that arrived while another ceremony was running.
 */
export function createCeremonies({ credentials, post }) {
  let busy = false;

  async function exclusive(kind, ceremony) {
    if (busy) return { ignored: true };
    busy = true;
    try {
      return await ceremony();
    } catch (error) {
      return { error: error instanceof Refused ? describeRefusal(kind, error.response) : describeError(kind, error) };
    } finally {
      busy = false;
    }
  }

  async function posted(path, fields) {
    const response = await post(path, fields);
    if (!response.ok) throw new Refused(response);
    return response.payload;
  }

  /** /auth/passkeys: options, create(), complete, as the stock page posts them. */
  function register(displayName) {
    return exclusive("register", async () => {
      const options = await posted("options", { display_name: displayName });
      const credential = await credentials.create({ publicKey: registrationOptions(options.options) });
      await posted("complete", {
        display_name: displayName,
        credential: JSON.stringify(serializeRegistration(credential)),
      });
      return { added: true };
    });
  }

  /** /auth/verify/<key>: options, get(), complete. */
  function verify() {
    return exclusive("verify", async () => {
      const options = await posted("options", {});
      const credential = await credentials.get({ publicKey: authenticationOptions(options.options) });
      const completion = await posted("complete", { credential: JSON.stringify(serializeAssertion(credential)) });
      return { redirect: completion.redirect || "/" };
    });
  }

  return { register, verify, isBusy: () => busy };
}

// --- the page -------------------------------------------------------------------

/**
 * POSTs to `${base}${path}` with the page's CSRF token. Skrift rotates the token on
 * every check it passes; its success answers carry the new one, but a refusal after
 * the check may not, which would leave every form on the page stale. So after any
 * answer without a token, the poster reads the current one from a fresh GET of this
 * page, which renders the session's token into its forms.
 */
export function csrfPoster({ document, fetchImpl, base, pageUrl }) {
  const inputs = () => document.querySelectorAll('input[name="_csrf"]');
  const store = (token) =>
    inputs().forEach((input) => {
      input.value = token;
    });

  async function refreshToken() {
    try {
      const page = await fetchImpl(pageUrl, { credentials: "same-origin" });
      const match = /name="_csrf" value="([^"]+)"/.exec(await page.text());
      if (match) store(match[1]);
    } catch {
      // Nothing to read; the next attempt will say so.
    }
  }

  return async function post(path, fields) {
    const body = new FormData();
    body.append("_csrf", inputs()[0]?.value || "");
    for (const [name, value] of Object.entries(fields)) body.append(name, value);
    const response = await fetchImpl(`${base}${path}`, { method: "POST", body, credentials: "same-origin" });
    const payload = await response.json().catch(() => ({}));
    if (typeof payload.csrf_token === "string" && payload.csrf_token) store(payload.csrf_token);
    else await refreshToken();
    return { ok: response.ok, status: response.status, payload };
  };
}

function bindForm({ form, status, busyText, doneText, run, onDone }) {
  const button = form.querySelector('button[type="submit"]');
  const say = (text, tone = "") => {
    status.textContent = text;
    status.dataset.tone = tone;
  };
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    button.disabled = true;
    form.setAttribute("aria-busy", "true");
    say(busyText);
    const outcome = await run();
    if (outcome.ignored) return;
    if (outcome.error) {
      button.disabled = false;
      form.removeAttribute("aria-busy");
      say(outcome.error, "error");
      return;
    }
    say(doneText);
    onDone(outcome);
  });
}

function unsupported(form, status) {
  form.querySelector('button[type="submit"]').disabled = true;
  status.textContent = "This browser can't use passkeys. Open Build in an up-to-date browser on your phone or computer.";
  status.dataset.tone = "error";
}

/** Binds whichever of the two pages this is; returns its ceremonies, or null. */
export function bindPasskeyPage(document, window) {
  const registration = document.getElementById("passkey-registration-form");
  const verification = document.getElementById("passkey-verify-form");
  const form = registration || verification;
  if (!form) return null;
  const status = document.getElementById(registration ? "passkey-status" : "verify-status");

  if (!window.PublicKeyCredential || !window.navigator.credentials) {
    unsupported(form, status);
    return null;
  }

  const base = registration ? "/auth/passkeys/" : `/auth/verify/${encodeURIComponent(form.dataset.factorKey)}/`;
  const ceremonies = createCeremonies({
    credentials: window.navigator.credentials,
    post: csrfPoster({ document, fetchImpl: window.fetch.bind(window), base, pageUrl: window.location.href }),
  });

  if (registration) {
    bindForm({
      form,
      status,
      busyText: "Creating your passkey…",
      doneText: "Passkey added. Reloading…",
      run: () => ceremonies.register(String(new FormData(form).get("display_name") || "")),
      onDone: () => window.location.reload(),
    });
  } else {
    bindForm({
      form,
      status,
      busyText: "Waiting for your passkey…",
      doneText: "Verified. Opening Build…",
      run: () => ceremonies.verify(),
      onDone: (outcome) => window.location.assign(outcome.redirect),
    });
  }
  return ceremonies;
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  bindPasskeyPage(document, window);
}
