import assert from "node:assert/strict";
import test from "node:test";

import {
  authorizationUrl,
  codeChallenge,
  parseAuthorizationCallback,
} from "../src/oauth.mjs";

test("PKCE S256 matches the RFC 7636 example", () => {
  assert.equal(
    codeChallenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
  );
});

test("the authorization request uses the registered public client and PKCE", () => {
  const url = new URL(authorizationUrl({ state: "state", challenge: "challenge" }));
  assert.equal(url.origin, "https://getbuild.ing");
  assert.equal(url.pathname, "/oauth/authorize");
  assert.equal(url.searchParams.get("client_id"), "build-desktop");
  assert.equal(url.searchParams.get("redirect_uri"), "getbuilding://oauth/callback");
  assert.equal(url.searchParams.get("response_type"), "code");
  assert.equal(url.searchParams.get("scope"), "openid profile email");
  assert.equal(url.searchParams.get("code_challenge_method"), "S256");
});

test("the callback requires the registered URL and matching state", () => {
  assert.deepEqual(
    parseAuthorizationCallback(
      "getbuilding://oauth/callback?code=secret-code&state=expected",
      "expected",
    ),
    { code: "secret-code" },
  );
  assert.throws(
    () => parseAuthorizationCallback("getbuilding://evil/callback?code=x&state=s", "s"),
    /callback URL/,
  );
  assert.throws(
    () => parseAuthorizationCallback("getbuilding://oauth/callback?code=x&state=wrong", "s"),
    /state/,
  );
});
