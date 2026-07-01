// Web-push helpers: VAPID application-server-key decoding must produce the exact
// bytes the push service expects (base64url, unpadded, 65-byte uncompressed point).

import { describe, expect, it } from "vitest";
import { urlBase64ToUint8Array } from "../src/push.js";

describe("urlBase64ToUint8Array", () => {
  it("decodes unpadded base64url", () => {
    // "hello" -> aGVsbG8 (unpadded)
    expect(Array.from(urlBase64ToUint8Array("aGVsbG8"))).toEqual([
      104, 101, 108, 108, 111,
    ]);
  });

  it("decodes url-safe characters (- and _)", () => {
    // 0xfb 0xff -> "+/8=" standard, "-_8" url-safe unpadded
    expect(Array.from(urlBase64ToUint8Array("-_8"))).toEqual([251, 255]);
  });

  it("round-trips a realistic 65-byte VAPID public key", () => {
    const bytes = new Uint8Array(65).map((_, i) => (i * 7 + 4) % 256);
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    const b64url = btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    expect(Array.from(urlBase64ToUint8Array(b64url))).toEqual(Array.from(bytes));
  });
});
