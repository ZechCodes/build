// Skrift session bootstrap for the Node harnesses (relay-direct topology).
//
// The relay's /ws/client requires a gateway token minted by the api
// (POST /api/gateway-token), which itself requires a logged-in Skrift session.
// This walks the dev dummy-login flow: fetch the login form (session cookie +
// CSRF token), submit it, then mint tokens with the resulting session.

function collectCookies(jar, response) {
  for (const line of response.headers.getSetCookie?.() || []) {
    const [pair] = line.split(";");
    const eq = pair.indexOf("=");
    if (eq > 0) jar.set(pair.slice(0, eq).trim(), pair.slice(eq + 1).trim());
  }
}

function cookieHeader(jar) {
  return [...jar.entries()].map(([name, value]) => `${name}=${value}`).join("; ");
}

/** Dummy-login to the api; returns { cookie, mintGatewayToken }. Dev-only. */
export async function loginWithDummy(apiUrl, { email = "qa@localhost", name = "QA" } = {}) {
  const jar = new Map();

  const formPage = await fetch(`${apiUrl}/auth/dummy/login`, { redirect: "manual" });
  collectCookies(jar, formPage);
  const html = await formPage.text();
  const csrf = html.match(/name="_csrf"\s+value="([^"]+)"/)?.[1];
  if (!csrf) throw new Error("no CSRF token on the dummy login form (dummy auth enabled?)");

  const submit = await fetch(`${apiUrl}/auth/dummy-login`, {
    method: "POST",
    redirect: "manual",
    headers: {
      "Content-Type": "application/x-www-form-urlencoded",
      Cookie: cookieHeader(jar),
    },
    body: new URLSearchParams({ _csrf: csrf, email, name }),
  });
  collectCookies(jar, submit);
  if (submit.status >= 400) throw new Error(`dummy login failed: HTTP ${submit.status}`);

  const cookie = cookieHeader(jar);

  async function mintGatewayToken() {
    const response = await fetch(`${apiUrl}/api/gateway-token`, {
      method: "POST",
      headers: { Cookie: cookie },
    });
    if (!response.ok) throw new Error(`gateway-token mint failed: HTTP ${response.status}`);
    return (await response.json()).token;
  }

  return { cookie, mintGatewayToken };
}
