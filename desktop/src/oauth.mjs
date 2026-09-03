import { createHash, randomBytes, timingSafeEqual } from "node:crypto";

export const CLIENT_ID = "build-desktop";
export const REDIRECT_URI = "getbuilding://oauth/callback";
export const TOKEN_URL = "https://getbuild.ing/oauth/token";

const base64url = (value) => Buffer.from(value).toString("base64url");

export function codeChallenge(verifier) {
  return base64url(createHash("sha256").update(verifier).digest());
}

export function createAuthorizationRequest() {
  const verifier = base64url(randomBytes(32));
  const state = base64url(randomBytes(32));
  return {
    verifier,
    state,
    url: authorizationUrl({ state, challenge: codeChallenge(verifier) }),
  };
}

export function authorizationUrl({ state, challenge }) {
  const url = new URL("https://getbuild.ing/oauth/authorize");
  url.search = new URLSearchParams({
    client_id: CLIENT_ID,
    redirect_uri: REDIRECT_URI,
    response_type: "code",
    scope: "openid profile email",
    state,
    code_challenge: challenge,
    code_challenge_method: "S256",
  });
  return url.toString();
}

export function parseAuthorizationCallback(value, expectedState) {
  const url = new URL(value);
  if (`${url.protocol}//${url.host}${url.pathname}` !== REDIRECT_URI) {
    throw new Error("invalid OAuth callback URL");
  }
  if (url.searchParams.has("error")) {
    throw new Error(url.searchParams.get("error_description") || url.searchParams.get("error"));
  }
  const actualState = url.searchParams.get("state") || "";
  const expected = Buffer.from(expectedState);
  const actual = Buffer.from(actualState);
  if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) {
    throw new Error("invalid OAuth state");
  }
  const code = url.searchParams.get("code");
  if (!code) throw new Error("OAuth callback did not include a code");
  return { code };
}

export function tokenRequestBody({ code, verifier }) {
  return new URLSearchParams({
    grant_type: "authorization_code",
    code,
    redirect_uri: REDIRECT_URI,
    client_id: CLIENT_ID,
    code_verifier: verifier,
  });
}
