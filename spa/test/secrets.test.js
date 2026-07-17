import { describe, it, expect } from "vitest";
import {
  isDotenvPath,
  isSecretLikeValue,
  isSecretLikeKeyName,
  maskDotenvLine,
  spoilerSpanHtml,
  renderDotenvSourceHtml,
  SPOILER_DOTS,
} from "../src/core/secrets.js";

describe("isDotenvPath", () => {
  it("matches dotenv basenames (.env, .env.*, *.envrc) at any depth", () => {
    expect(isDotenvPath(".env")).toBe(true);
    expect(isDotenvPath(".env.local")).toBe(true);
    expect(isDotenvPath(".env.production")).toBe(true);
    expect(isDotenvPath("config/.env")).toBe(true);
    expect(isDotenvPath("apps/web/.env.test")).toBe(true);
    expect(isDotenvPath(".envrc")).toBe(true);
    expect(isDotenvPath("prod.envrc")).toBe(true);
  });

  it("leaves non-dotenv files (and lookalikes) visible", () => {
    expect(isDotenvPath("src/env.rs")).toBe(false);
    expect(isDotenvPath("foo.env")).toBe(false); // trailing .env is not a dotenv basename
    expect(isDotenvPath(".environment")).toBe(false);
    expect(isDotenvPath("README.md")).toBe(false);
    expect(isDotenvPath("")).toBe(false);
    expect(isDotenvPath(null)).toBe(false);
  });
});

describe("isSecretLikeValue (shape rule)", () => {
  it("masks high-entropy tokens (len>=20, secret charset, >=1 digit)", () => {
    // JWT: base64url segments joined by dots
    expect(isSecretLikeValue("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w")).toBe(true);
    // sk- style key
    expect(isSecretLikeValue("sk-abcdef0123456789ABCDEF")).toBe(true);
    // ghp_ style key (underscore is in the charset)
    expect(isSecretLikeValue("ghp_16CharsAndMore0123456789")).toBe(true);
    // hex key
    expect(isSecretLikeValue("0a1b2c3d4e5f60718293a4b5c6d7e8f9")).toBe(true);
    // base64 with + and = padding
    expect(isSecretLikeValue("YWJjZGVmZ2hp+jklm1234==")).toBe(true);
  });

  it("leaves human-readable / structured values visible", () => {
    expect(isSecretLikeValue("auth.example.com")).toBe(false); // no digit
    expect(isSecretLikeValue("https://example.com:8080/path")).toBe(false); // colon+slash break the charset
    expect(isSecretLikeValue("word-chain-without-digits")).toBe(false); // no digit
    expect(isSecretLikeValue("a1234567890123456789")).toBe(true); // 20 chars, has digit
    expect(isSecretLikeValue("a123456789012345678")).toBe(false); // 19 chars — too short
    expect(isSecretLikeValue("abcdefghijklmnopqrst")).toBe(false); // 20 chars but no digit
    expect(isSecretLikeValue("has/slash/12345678901234")).toBe(false); // slash excluded (paths)
    expect(isSecretLikeValue("")).toBe(false);
  });
});

describe("isSecretLikeKeyName (name rule)", () => {
  it("masks when any underscore segment is a secret word (case-insensitive)", () => {
    expect(isSecretLikeKeyName("PRIVATE_KEY")).toBe(true);
    expect(isSecretLikeKeyName("DB_PASS")).toBe(true);
    expect(isSecretLikeKeyName("API_TOKEN")).toBe(true);
    expect(isSecretLikeKeyName("GHCR_PASSWORD")).toBe(true);
    expect(isSecretLikeKeyName("SSH_PASSPHRASE")).toBe(true);
    expect(isSecretLikeKeyName("MY_APIKEY")).toBe(true);
    expect(isSecretLikeKeyName("APP_SECRET")).toBe(true);
    expect(isSecretLikeKeyName("USER_PASSWD")).toBe(true);
    expect(isSecretLikeKeyName("db_pass")).toBe(true); // case-insensitive
    expect(isSecretLikeKeyName("PWD")).toBe(true);
    expect(isSecretLikeKeyName("REDIS_CREDENTIALS")).toBe(true);
    expect(isSecretLikeKeyName("A_CREDENTIAL")).toBe(true);
    expect(isSecretLikeKeyName("A_PASSPHRASE")).toBe(true);
  });

  it("is segment matching, not substring — lookalikes stay visible", () => {
    expect(isSecretLikeKeyName("KEYCLOAK_HOST")).toBe(false); // KEYCLOAK != KEY
    expect(isSecretLikeKeyName("COMPASS_URL")).toBe(false); // COMPASS contains PASS as substring only
    expect(isSecretLikeKeyName("PASSAGE_ID")).toBe(false);
    expect(isSecretLikeKeyName("DATABASE_URL")).toBe(false);
    expect(isSecretLikeKeyName("PORT")).toBe(false);
  });
});

describe("maskDotenvLine — shape-based", () => {
  it("masks a JWT value", () => {
    const jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w";
    const seg = maskDotenvLine(`JWT=${jwt}`);
    expect(seg.masked).toBe(true);
    expect(seg.prefix).toBe("JWT=");
    expect(seg.value).toBe(jwt);
    expect(seg.suffix).toBe("");
  });

  it("masks an sk- api key", () => {
    const seg = maskDotenvLine("OPENAI_API_KEY=sk-abcdef0123456789ABCDEF");
    expect(seg.masked).toBe(true);
    expect(seg.value).toBe("sk-abcdef0123456789ABCDEF");
  });

  it("strips one pair of double quotes into prefix/suffix", () => {
    const seg = maskDotenvLine('TOKEN="abcdef0123456789ghij"');
    expect(seg.masked).toBe(true);
    expect(seg.prefix).toBe('TOKEN="');
    expect(seg.value).toBe("abcdef0123456789ghij");
    expect(seg.suffix).toBe('"');
    expect(seg.prefix + seg.value + seg.suffix).toBe('TOKEN="abcdef0123456789ghij"');
  });

  it("strips one pair of single quotes", () => {
    const seg = maskDotenvLine("TOKEN='abcdef0123456789ghij'");
    expect(seg.prefix).toBe("TOKEN='");
    expect(seg.value).toBe("abcdef0123456789ghij");
    expect(seg.suffix).toBe("'");
  });

  it("handles a leading `export ` prefix", () => {
    const seg = maskDotenvLine("export SECRET_TOKEN=abcdef0123456789ghij");
    expect(seg.masked).toBe(true);
    expect(seg.prefix).toBe("export SECRET_TOKEN=");
    expect(seg.value).toBe("abcdef0123456789ghij");
  });

  it("keeps an inline comment after an unquoted value out of the value", () => {
    const seg = maskDotenvLine("APIKEY=abcdef0123456789ghij # production key");
    expect(seg.masked).toBe(true);
    expect(seg.value).toBe("abcdef0123456789ghij");
    expect(seg.suffix).toBe(" # production key");
    expect(seg.prefix + seg.value + seg.suffix).toBe("APIKEY=abcdef0123456789ghij # production key");
  });

  it("masks base64 with + and = padding", () => {
    const seg = maskDotenvLine("B64=YWJjZGVmZ2hp+jklm1234==");
    expect(seg.masked).toBe(true);
    expect(seg.value).toBe("YWJjZGVmZ2hp+jklm1234==");
  });

  it("leaves short, no-digit, hostname, and URL values visible", () => {
    expect(maskDotenvLine("SHORT=a123456789012345678").masked).toBe(false); // 19 chars
    expect(maskDotenvLine("WORDS=abcdefghijklmnopqrst").masked).toBe(false); // 20, no digit
    expect(maskDotenvLine("HOST=auth.example.com").masked).toBe(false);
    expect(maskDotenvLine("URL=https://example.com:8080/x").masked).toBe(false);
  });

  it("returns unmasked for empty values, comments, and bare KEY=", () => {
    expect(maskDotenvLine("EMPTY=").masked).toBe(false);
    expect(maskDotenvLine("QUOTED_EMPTY=''").masked).toBe(false);
    expect(maskDotenvLine("# just a comment").masked).toBe(false);
    expect(maskDotenvLine("   # indented comment").masked).toBe(false);
    expect(maskDotenvLine("no equals here").masked).toBe(false);
    expect(maskDotenvLine("").masked).toBe(false);
  });
});

describe("maskDotenvLine — name-based", () => {
  it("masks ANY non-empty value when the key name is secret-like", () => {
    const seg = maskDotenvLine("DB_PASS=hunter2!"); // short + symbol → shape rule alone would skip it
    expect(seg.masked).toBe(true);
    expect(seg.value).toBe("hunter2!");
    expect(seg.prefix).toBe("DB_PASS=");
  });

  it("masks a short PRIVATE_KEY value", () => {
    const seg = maskDotenvLine("PRIVATE_KEY=short");
    expect(seg.masked).toBe(true);
    expect(seg.value).toBe("short");
  });

  it("is case-insensitive on the key name", () => {
    expect(maskDotenvLine("db_pass=whatever").masked).toBe(true);
  });

  it("leaves name-lookalikes visible", () => {
    expect(maskDotenvLine("KEYCLOAK_HOST=auth.example.com").masked).toBe(false);
    expect(maskDotenvLine("COMPASS_URL=https://compass.example.com").masked).toBe(false);
  });

  it("renders a name-matched key with an empty value unmasked (nothing to hide)", () => {
    expect(maskDotenvLine("API_KEY=").masked).toBe(false);
  });
});

describe("spoilerSpanHtml (diff path — escaped value in data attr)", () => {
  it("shows fixed-width dots and never leaks value length", () => {
    const html = spoilerSpanHtml("abcdef0123456789ghij");
    expect(html).toContain(SPOILER_DOTS);
    expect(html).toContain('class="spoiler"');
    // Fixed 10 dots regardless of the real length.
    expect(SPOILER_DOTS.length).toBe(10);
  });

  it("escapes an HTML-bearing value (no XSS in data-secret)", () => {
    const html = spoilerSpanHtml("<script>alert(1)</script>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });
});

describe("renderDotenvSourceHtml (files path — value NOT in DOM)", () => {
  it("keeps masked values out of the HTML and collects them in the secrets array", () => {
    const { html, secrets } = renderDotenvSourceHtml(
      ["HOST=auth.example.com", "TOKEN=abcdef0123456789ghij"].join("\n"),
    );
    expect(html).toContain("auth.example.com"); // visible line stays
    expect(html).not.toContain("abcdef0123456789ghij"); // secret is NOT in the DOM string
    expect(html).toContain(SPOILER_DOTS);
    expect(html).toContain('data-secret-index="0"');
    expect(secrets).toEqual(["abcdef0123456789ghij"]);
  });

  it("escapes both visible lines and (name-masked) hostile values", () => {
    const { html, secrets } = renderDotenvSourceHtml(
      ["# <script>c</script>", 'PASSWORD="<img src=x onerror=alert(1)>"'].join("\n"),
    );
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
    // The hostile value lives only in JS state, never rendered into the DOM string.
    expect(secrets).toEqual(["<img src=x onerror=alert(1)>"]);
    expect(html).not.toContain("onerror=alert(1)");
  });

  it("returns an empty secrets list for a file with nothing to hide", () => {
    const { secrets } = renderDotenvSourceHtml("HOST=localhost\nPORT=8080\n");
    expect(secrets).toEqual([]);
  });
});
