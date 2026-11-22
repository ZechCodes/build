import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  launch,
  response,
  tokens,
  flush,
} from "./helpers/main-auth-harness.mjs";

function bridgeFetch(requests, offline = false) {
  return async (url, options) => {
    requests.push({ url, options });
    if (options.method === "DELETE") {
      if (offline) throw new Error("offline");
      return new Response(null, { status: 204 });
    }
    return new Response(
      JSON.stringify({
        issuer: "https://getbuild.ing",
        account_id: "account",
        device_id: "device",
        origin: "https://bridge.test",
        endpoint_epoch: "epoch",
        family_id: "family",
        access_token: "fixture-device-token",
        expires_at: Math.floor(Date.now() / 1000) + 300,
        scope: "bridge:read bridge:write",
      }),
    );
  };
}
const resume = {
  deviceId: "device",
  origin: "https://bridge.test",
  endpointEpoch: "epoch",
  familyId: "family",
};
function invoke(h, method, args) {
  const contents = h.calls.windows.at(-1).webContents;
  return h.calls.ipc.get("build:bridge-http")(
    { sender: contents, senderFrame: contents.mainFrame },
    method,
    args,
  );
}

function guardWindowContents(window) {
  const contents = window.webContents;
  Object.defineProperty(window, "webContents", {
    get() {
      if (window.isDestroyed()) throw new TypeError("Object has been destroyed");
      return contents;
    },
  });
  return contents;
}

test("main installs only sandboxed typed preload and fences old handles on OAuth rotation", async (t) => {
  const requests = [];
  const h = await launch(t, {
    bridgeFetch: bridgeFetch(requests),
    fetch: (count) => response(tokens(count === 1 ? "initial" : "replacement")),
  });
  assert.equal(
    h.calls.windows[0].options.webPreferences.preload,
    fileURLToPath(new URL("../src/preload.cjs", import.meta.url)),
  );
  const first = await invoke(h, "resume", [resume]);
  assert.equal(first.ok, true);
  await h.refreshTokens();
  await flush();
  const revoked = requests.find((r) => r.options.method === "DELETE");
  assert.equal(
    revoked.url,
    "https://getbuild.ing/api/devices/device/http-credentials/family",
  );
  assert.equal(
    revoked.options.headers.Authorization,
    "Bearer fixture-access-initial",
  );
  assert.equal(h.header(), "Bearer fixture-access-replacement");
  assert.equal((await invoke(h, "open", [first.value.handle, "1"])).ok, false);
  assert.equal((await invoke(h, "resume", [resume])).ok, true);
});

test("offline native signout clears local credentials and retires IPC even when family revoke fails", async (t) => {
  const requests = [];
  const h = await launch(t, { bridgeFetch: bridgeFetch(requests, true) });
  assert.equal((await invoke(h, "resume", [resume])).ok, true);
  await h.signOut().click();
  await flush();
  assert.equal(h.header(), undefined);
  assert.equal(h.calls.windows[0].destroyed, true);
  assert.equal(requests.filter((r) => r.options.method === "DELETE").length, 1);
  assert.ok(h.calls.quit);
});

test("native sign-out completes when closed fires after Electron destroys the window", async (t) => {
  const requests = [];
  const h = await launch(t, { bridgeFetch: bridgeFetch(requests) });
  const [window] = h.calls.windows;
  const contents = guardWindowContents(window);
  assert.equal((await invoke(h, "resume", [resume])).ok, true);
  window.prependOnceListener("closed", () => contents.emit("destroyed"));

  await h.signOut().click();

  assert.equal(window.destroyed, true);
  assert.equal(requests.filter((r) => r.options.method === "DELETE").length, 1);
  assert.deepEqual(h.calls.cleanup, ["storage", "cache"]);
  assert.equal(h.calls.quit, 1);
});

test("late document and IPC callbacks ignore the destroyed window during close", async (t) => {
  const requests = [];
  const h = await launch(t, { bridgeFetch: bridgeFetch(requests) });
  const [window] = h.calls.windows;
  const contents = guardWindowContents(window);
  const { id, mainFrame } = contents;
  const ipc = h.calls.ipc.get("build:bridge-http");
  let lateResponse;
  let lateCall;
  window.prependOnceListener("closed", () => {
    h.calls.responseHeaders({
      url: mainFrame.url, statusCode: 401, resourceType: "mainFrame", webContentsId: id,
    }, (decision) => { lateResponse = decision; });
    lateCall = ipc({ sender: contents, senderFrame: mainFrame }, "resume", [resume]);
  });

  window.destroy();

  assert.deepEqual(lateResponse, {});
  assert.equal((await lateCall).error.code, "untrusted_caller");
  assert.deepEqual(requests, []);
  h.app.emit("activate");
  assert.equal(h.calls.windows.length, 2);
});

test("main IPC uses the actual packaged manifest for a body-free GET resource", async (t) => {
  const { default: manifest } = await import(
    "../src/bridge-http-resources.json",
    { with: { type: "json" } }
  );
  const requests = [];
  const hosted = bridgeFetch(requests);
  const h = await launch(t, {
    bridgeFetch: async (url, options) => {
      if (url === "https://bridge.test/v1/sessions")
        return new Response(
          JSON.stringify({
            session_id: "session",
            opening_generation: "generation",
            grant_expires_at: new Date(Date.now() + 30000).toISOString(),
            greeting: { bridge_epoch: "epoch" },
            transport: manifest.transport,
          }),
        );
      if (url === "https://bridge.test/v1/projects") {
        assert.equal(options.method, "GET");
        assert.equal(options.body, undefined);
        return new Response(
          JSON.stringify({
            id: "request",
            ok: true,
            result: { projects: [] },
            bridge_epoch: "epoch",
          }),
        );
      }
      return hosted(url, options);
    },
  });
  const identity = await invoke(h, "resume", [resume]);
  assert.equal(identity.ok, true);
  assert.equal(
    (await invoke(h, "open", [identity.value.handle, "1"])).ok,
    true,
  );
  const result = await invoke(h, "request", [
    identity.value.handle,
    "project_list",
    {},
    { requestId: "request" },
  ]);
  assert.deepEqual(result, {
    ok: true,
    value: {
      id: "request",
      ok: true,
      result: { projects: [] },
      bridge_epoch: "epoch",
    },
  });
});
