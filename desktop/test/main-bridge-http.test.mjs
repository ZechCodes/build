import assert from "node:assert/strict";
import test from "node:test";
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

test("main installs only sandboxed typed preload and fences old handles on OAuth rotation", async (t) => {
  const requests = [];
  const h = await launch(t, {
    bridgeFetch: bridgeFetch(requests),
    fetch: (count) => response(tokens(count === 1 ? "initial" : "replacement")),
  });
  assert.ok(
    h.calls.windows[0].options.webPreferences.preload.endsWith("/preload.cjs"),
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
